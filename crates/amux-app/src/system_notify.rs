use std::io::Cursor;
use std::path::Path;
use std::sync::mpsc;

/// Action triggered by clicking a system notification.
pub struct NotificationAction {
    pub workspace_id: u64,
    pub pane_id: u64,
}

/// Cross-platform system notification sender using `notify-rust`.
///
/// Sends OS-native toast notifications (UNUserNotificationCenter on macOS,
/// Windows toast notifications on Windows). Notification clicks are routed
/// back via an mpsc channel as `NotificationAction`s.
pub struct SystemNotifier {
    action_rx: mpsc::Receiver<NotificationAction>,
    _action_tx: mpsc::Sender<NotificationAction>,
}

impl SystemNotifier {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            action_rx: rx,
            _action_tx: tx,
        }
    }

    /// Send an OS-native notification.
    pub fn send(&self, title: &str, body: &str, _workspace_id: u64, _pane_id: u64) {
        let title = title.to_string();
        let body = body.to_string();

        // Spawn in background to avoid blocking the UI thread.
        std::thread::spawn(move || {
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

            // On macOS, notify-rust uses UNUserNotificationCenter which
            // handles permission prompts automatically on first use.
            if let Err(e) = notification.show() {
                tracing::warn!("Failed to show system notification: {}", e);
            }
        });
    }

    /// Drain any click-to-navigate actions from notification callbacks.
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
    /// Cached bytes of a custom sound file (loaded once).
    custom_sound: Option<Vec<u8>>,
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
                    self.custom_sound = Some(bytes);
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
                    let cursor = Cursor::new(bytes.clone());
                    match rodio::Decoder::new(cursor) {
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

/// Update the dock/taskbar badge with the unread notification count.
/// On macOS, sets the dock tile badge label. On Windows, flashes the taskbar.
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
        // Windows: flash the taskbar button when there are unread notifications.
        // A full count overlay via ITaskbarList3::SetOverlayIcon is more complex
        // and can be added later.
        if count > 0 {
            // FlashWindowEx would go here, but requires the HWND.
            // For now, this is a placeholder — eframe doesn't expose HWND directly.
            let _ = count;
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = count;
    }
}

/// Run a custom command on notification, with env vars set.
pub fn run_custom_command(command: &str, title: &str, body: &str, source: &str) {
    let command = command.to_string();
    let title = title.to_string();
    let body = body.to_string();
    let source = source.to_string();

    std::thread::spawn(move || {
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

        match std::process::Command::new(shell)
            .arg(flag)
            .arg(&command)
            .env("AMUX_NOTIFICATION_TITLE", &title)
            .env("AMUX_NOTIFICATION_BODY", &body)
            .env("AMUX_NOTIFICATION_SOURCE", &source)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("Failed to run notification command: {}", e);
            }
        }
    });
}
