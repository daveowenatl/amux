use std::path::Path;
use std::sync::{mpsc, Arc};

/// Action triggered by clicking a system notification.
pub struct NotificationAction {
    pub workspace_id: u64,
    pub pane_id: u64,
}

/// Message sent to the background worker thread.
enum WorkerMsg {
    ShowNotification {
        title: String,
        body: String,
    },
    RunCommand {
        command: String,
        title: String,
        body: String,
        source: String,
    },
}

/// Cross-platform system notification sender using `notify-rust`.
///
/// Sends OS-native toast notifications (UNUserNotificationCenter on macOS,
/// Windows toast notifications on Windows). Uses a single background worker
/// thread to avoid spawning unbounded threads on notification bursts.
///
/// Note: click-to-navigate actions are plumbed but not yet produced —
/// `notify-rust` doesn't expose a cross-platform callback for notification
/// clicks. The `drain_actions()` / action channel is scaffolding for when
/// we add platform-specific click handling (or fall back to spawning
/// `amux focus` as a subprocess).
pub struct SystemNotifier {
    action_rx: mpsc::Receiver<NotificationAction>,
    _action_tx: mpsc::Sender<NotificationAction>,
    worker_tx: mpsc::Sender<WorkerMsg>,
}

impl SystemNotifier {
    pub fn new() -> Self {
        let (action_tx, action_rx) = mpsc::channel();
        let (worker_tx, worker_rx) = mpsc::channel::<WorkerMsg>();

        // Single long-lived worker thread for notifications and commands.
        std::thread::Builder::new()
            .name("amux-notify-worker".into())
            .spawn(move || {
                for msg in worker_rx {
                    match msg {
                        WorkerMsg::ShowNotification { title, body } => {
                            let mut notification = notify_rust::Notification::new();
                            notification.appname("amux");
                            if title.is_empty() {
                                notification.summary("amux");
                            } else {
                                notification.summary(&title);
                            }
                            if !body.is_empty() {
                                notification.body(&body);
                            }
                            if let Err(e) = notification.show() {
                                tracing::warn!("Failed to show system notification: {}", e);
                            }
                        }
                        WorkerMsg::RunCommand {
                            command,
                            title,
                            body,
                            source,
                        } => {
                            let shell = if cfg!(target_os = "windows") {
                                "cmd"
                            } else {
                                "sh"
                            };
                            let flag = if cfg!(target_os = "windows") {
                                "/C"
                            } else {
                                "-c"
                            };
                            // Use .status() instead of .spawn() to wait for
                            // and reap the child process, avoiding zombies.
                            match std::process::Command::new(shell)
                                .arg(flag)
                                .arg(&command)
                                .env("AMUX_NOTIFICATION_TITLE", &title)
                                .env("AMUX_NOTIFICATION_BODY", &body)
                                .env("AMUX_NOTIFICATION_SOURCE", &source)
                                .stdin(std::process::Stdio::null())
                                .stdout(std::process::Stdio::null())
                                .stderr(std::process::Stdio::null())
                                .status()
                            {
                                Ok(_) => {}
                                Err(e) => {
                                    tracing::warn!("Failed to run notification command: {}", e);
                                }
                            }
                        }
                    }
                }
            })
            .expect("failed to spawn notification worker thread");

        Self {
            action_rx,
            _action_tx: action_tx,
            worker_tx,
        }
    }

    /// Send an OS-native notification via the background worker.
    pub fn send(&self, title: &str, body: &str, _workspace_id: u64, _pane_id: u64) {
        let _ = self.worker_tx.send(WorkerMsg::ShowNotification {
            title: title.to_string(),
            body: body.to_string(),
        });
    }

    /// Run a custom command on notification via the background worker.
    pub fn run_custom_command(&self, command: &str, title: &str, body: &str, source: &str) {
        let _ = self.worker_tx.send(WorkerMsg::RunCommand {
            command: command.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            source: source.to_string(),
        });
    }

    /// Drain any click-to-navigate actions from notification callbacks.
    /// Currently always empty — click handling is not yet implemented.
    pub fn drain_actions(&self) -> Vec<NotificationAction> {
        self.action_rx.try_iter().collect()
    }
}

/// Cross-platform notification sound player using `rodio`.
///
/// Holds the audio output stream alive for the app lifetime and plays
/// sounds on demand. Supports "system" (short beep), "none" (silent),
/// or a path to a custom audio file (wav/ogg/mp3).
pub struct SoundPlayer {
    _stream: rodio::OutputStream,
    stream_handle: rodio::OutputStreamHandle,
    /// Cached bytes of a custom sound file (loaded once, shared via Arc).
    custom_sound: Option<Arc<[u8]>>,
    /// Current sound mode.
    mode: SoundMode,
}

#[derive(Clone)]
enum SoundMode {
    System,
    None,
    Custom,
}

/// A short 440Hz sine wave beep (~150ms) generated at runtime.
fn system_beep_source() -> rodio::source::SineWave {
    rodio::source::SineWave::new(440.0)
}

impl SoundPlayer {
    /// Create a new sound player. Returns `None` if no audio device is available.
    pub fn new() -> Option<Self> {
        match rodio::OutputStream::try_default() {
            Ok((stream, handle)) => Some(Self {
                _stream: stream,
                stream_handle: handle,
                custom_sound: None,
                mode: SoundMode::System,
            }),
            Err(e) => {
                tracing::warn!("No audio output device: {}", e);
                None
            }
        }
    }

    /// Configure the sound player from config.
    /// `sound` is "system", "none", or a file path.
    pub fn configure(&mut self, sound: &str) {
        if sound == "none" {
            self.mode = SoundMode::None;
            self.custom_sound = None;
        } else if sound == "system" {
            self.mode = SoundMode::System;
            self.custom_sound = None;
        } else {
            // Treat as file path
            let path = Path::new(sound);
            match std::fs::read(path) {
                Ok(bytes) => {
                    self.custom_sound = Some(bytes.into());
                    self.mode = SoundMode::Custom;
                    tracing::info!("Loaded custom notification sound: {}", path.display());
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to load custom sound {}: {} — falling back to system",
                        path.display(),
                        e
                    );
                    self.mode = SoundMode::System;
                    self.custom_sound = None;
                }
            }
        }
    }

    /// Play the configured notification sound.
    pub fn play(&self) {
        match &self.mode {
            SoundMode::None => {}
            SoundMode::System => {
                use rodio::Source;
                let beep = system_beep_source()
                    .take_duration(std::time::Duration::from_millis(150))
                    .amplify(0.3);
                if let Err(e) = self.stream_handle.play_raw(beep.convert_samples()) {
                    tracing::warn!("Failed to play system beep: {}", e);
                }
            }
            SoundMode::Custom => {
                if let Some(bytes) = &self.custom_sound {
                    // Arc clone is cheap — just bumps reference count.
                    let reader = ArcSliceReader::new(Arc::clone(bytes));
                    match rodio::Decoder::new(reader) {
                        Ok(source) => {
                            if let Err(e) = self
                                .stream_handle
                                .play_raw(rodio::Source::convert_samples(source))
                            {
                                tracing::warn!("Failed to play custom sound: {}", e);
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to decode custom sound: {}", e);
                        }
                    }
                }
            }
        }
    }
}

/// Wrapper around `Arc<[u8]>` that implements `Read` + `Seek` for rodio.
struct ArcSliceReader {
    data: Arc<[u8]>,
    pos: usize,
}

impl ArcSliceReader {
    fn new(data: Arc<[u8]>) -> Self {
        Self { data, pos: 0 }
    }
}

impl std::io::Read for ArcSliceReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let remaining = &self.data[self.pos..];
        let n = remaining.len().min(buf.len());
        buf[..n].copy_from_slice(&remaining[..n]);
        self.pos += n;
        Ok(n)
    }
}

impl std::io::Seek for ArcSliceReader {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        let len = self.data.len() as i64;
        let new_pos = match pos {
            std::io::SeekFrom::Start(p) => p as i64,
            std::io::SeekFrom::End(p) => len + p,
            std::io::SeekFrom::Current(p) => self.pos as i64 + p,
        };
        if new_pos < 0 || new_pos > len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "seek out of bounds",
            ));
        }
        self.pos = new_pos as usize;
        Ok(self.pos as u64)
    }
}

/// Update the dock/taskbar badge with the unread notification count.
/// On macOS, sets the dock tile badge label. On Windows, this is currently
/// a no-op — taskbar badge support requires HWND access not yet available.
/// Pass 0 to clear the badge.
pub fn set_badge_count(count: usize) {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::NSApplication;
        use objc2_foundation::{MainThreadMarker, NSString};

        // This is called from update() which runs on the main thread in eframe.
        if let Some(mtm) = MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            let dock_tile = app.dockTile();
            let label = if count == 0 {
                NSString::from_str("")
            } else if count > 99 {
                NSString::from_str("99+")
            } else {
                NSString::from_str(&count.to_string())
            };
            dock_tile.setBadgeLabel(Some(&label));
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Windows taskbar badge is not yet supported — requires HWND access
        // which eframe doesn't expose directly. FlashWindowEx or
        // ITaskbarList3::SetOverlayIcon can be added once we have a handle.
        let _ = count;
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = count;
    }
}
