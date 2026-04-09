/// Notification events parsed from OSC sequences intercepted by the terminal.
#[derive(Debug, Clone)]
pub enum NotificationEvent {
    /// OSC 9 / toast notification
    Toast { title: Option<String>, body: String },
    /// Bell character (\x07)
    Bell,
    /// Working directory changed (OSC 7)
    WorkingDirectoryChanged,
    /// Window title changed (OSC 0/2)
    TitleChanged(String),
}
