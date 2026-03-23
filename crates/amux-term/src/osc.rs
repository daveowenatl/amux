use std::sync::mpsc;

use wezterm_term::terminal::{Alert, AlertHandler};

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

/// Alert handler that forwards wezterm-term alerts into an mpsc channel
/// for consumption by the pane owner.
pub struct ChannelAlertHandler {
    tx: mpsc::Sender<NotificationEvent>,
}

impl ChannelAlertHandler {
    pub fn new(tx: mpsc::Sender<NotificationEvent>) -> Self {
        Self { tx }
    }
}

impl AlertHandler for ChannelAlertHandler {
    fn alert(&mut self, alert: Alert) {
        let event = match alert {
            Alert::Bell => Some(NotificationEvent::Bell),
            Alert::ToastNotification { title, body, .. } => {
                Some(NotificationEvent::Toast { title, body })
            }
            Alert::CurrentWorkingDirectoryChanged => {
                Some(NotificationEvent::WorkingDirectoryChanged)
            }
            Alert::WindowTitleChanged(title) => Some(NotificationEvent::TitleChanged(title)),
            _ => None,
        };
        if let Some(event) = event {
            let _ = self.tx.send(event);
        }
    }
}
