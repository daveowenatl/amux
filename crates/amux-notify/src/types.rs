//! Public notification data types.
//!
//! Defines the enums and structs that make up the notification domain:
//! agent state, notification source, flash reason, workspace status,
//! notification payload, and per-pane notification state.

use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Agent status state for a workspace sidebar pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    Idle,
    Active,
    Waiting,
}

/// What triggered the notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotificationSource {
    Toast,
    Bell,
    Cli,
}

impl NotificationSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Toast => "toast",
            Self::Bell => "bell",
            Self::Cli => "cli",
        }
    }
}

/// Why a pane is flashing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashReason {
    /// A notification just arrived (blue).
    NotificationArrival,
    /// A notification was dismissed (blue).
    NotificationDismiss,
}

/// Per-workspace agent status displayed as a pill in the sidebar.
#[derive(Debug, Clone)]
pub struct WorkspaceStatus {
    pub state: AgentState,
    pub label: Option<String>,
    pub updated_at: Instant,
    /// Optional progress value (0.0–1.0) for progress bar display.
    pub progress: Option<f32>,
    /// Agent's current task description (shown as title line in sidebar).
    pub task: Option<String>,
    /// Agent's latest message (shown as subtitle in sidebar).
    pub message: Option<String>,
}

/// A single notification entry.
#[derive(Debug, Clone)]
pub struct Notification {
    pub id: u64,
    pub workspace_id: u64,
    pub pane_id: u64,
    pub surface_id: u64,
    pub title: String,
    pub subtitle: String,
    pub body: String,
    pub source: NotificationSource,
    pub created_at: Instant,
    pub read: bool,
}

/// Per-pane notification visual state (ring + flash).
#[derive(Debug, Default)]
pub struct PaneNotifyState {
    pub unread_count: usize,
    pub flash_started_at: Option<Instant>,
    pub flash_reason: Option<FlashReason>,
}
