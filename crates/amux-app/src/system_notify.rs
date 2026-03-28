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
