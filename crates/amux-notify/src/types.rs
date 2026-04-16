//! Public notification data types.
//!
//! Defines the enums and structs that make up the notification domain:
//! agent state, notification source, flash reason, workspace status,
//! notification payload, and per-pane notification state.

use std::collections::BTreeMap;
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

/// Priority tiers for status entries. Higher values sort first in the sidebar.
///
/// Reserved keys under `agent.*` feed the existing sidebar fields (label,
/// task, message) so that legacy callers of [`super::NotificationStore::set_status`]
/// keep working bit-identically. Integrations (Claude hooks, Gemini, Codex,
/// user CLI) publish under their own namespaced keys with their own priority.
pub mod priority {
    /// Agent state label (e.g. "Running", "Needs input"). Always renders in
    /// the status row just below the title.
    pub const LABEL: i32 = 100;
    /// Agent's current task description (renders in the title line).
    pub const TASK: i32 = 80;
    /// Agent's latest message (renders as the subtitle under the status row).
    pub const MESSAGE: i32 = 60;
    /// User-published status without an explicit priority.
    pub const USER_GENERIC: i32 = 50;
}

/// A single keyed status entry published by a hook, integration, or CLI call.
///
/// Entries live on [`WorkspaceStatus::entries`] keyed by publisher (e.g.
/// `"agent.message"`, `"claude.tool"`, `"git.branch"`). A fresh write to the
/// same key replaces the prior entry; a call to
/// [`super::NotificationStore::remove_entry`] expires it.
#[derive(Debug, Clone)]
pub struct StatusEntry {
    pub key: String,
    pub text: String,
    pub priority: i32,
    pub icon: Option<String>,
    pub color: Option<[u8; 4]>,
    pub updated_at: Instant,
}

/// Per-workspace agent status displayed as a pill in the sidebar.
///
/// Stores agent state + progress as first-class meta fields, and a keyed
/// dictionary of [`StatusEntry`] rows. Legacy label/task/message are now
/// looked up through the reserved keys in [`priority`]; see
/// [`Self::label`], [`Self::task`], [`Self::message`].
#[derive(Debug, Clone)]
pub struct WorkspaceStatus {
    pub state: AgentState,
    pub updated_at: Instant,
    /// Optional progress value (0.0–1.0) for progress bar display.
    pub progress: Option<f32>,
    /// Keyed status entries. Use [`Self::entries_by_priority`] for the
    /// ordered render list; [`Self::label`] / [`Self::task`] /
    /// [`Self::message`] for the three legacy sidebar slots.
    pub entries: BTreeMap<String, StatusEntry>,
}

/// Reserved entry keys used by the legacy sidebar view. Named here rather
/// than inline so renames are a single edit.
pub const KEY_AGENT_LABEL: &str = "agent.label";
pub const KEY_AGENT_TASK: &str = "agent.task";
pub const KEY_AGENT_MESSAGE: &str = "agent.message";

impl WorkspaceStatus {
    /// Fetch an entry by key.
    pub fn entry(&self, key: &str) -> Option<&StatusEntry> {
        self.entries.get(key)
    }

    /// Legacy: agent state label (e.g. "Running"). Backed by
    /// [`KEY_AGENT_LABEL`].
    pub fn label(&self) -> Option<&str> {
        self.entries.get(KEY_AGENT_LABEL).map(|e| e.text.as_str())
    }

    /// Legacy: agent task (rendered with ★ prefix in the title). Backed by
    /// [`KEY_AGENT_TASK`].
    pub fn task(&self) -> Option<&str> {
        self.entries.get(KEY_AGENT_TASK).map(|e| e.text.as_str())
    }

    /// Legacy: agent's latest message (sidebar subtitle). Backed by
    /// [`KEY_AGENT_MESSAGE`].
    pub fn message(&self) -> Option<&str> {
        self.entries.get(KEY_AGENT_MESSAGE).map(|e| e.text.as_str())
    }

    /// All entries sorted by descending priority, then by key for stable
    /// output on ties. The sidebar will iterate this once G20 lands.
    pub fn entries_by_priority(&self) -> Vec<&StatusEntry> {
        let mut v: Vec<&StatusEntry> = self.entries.values().collect();
        v.sort_by(|a, b| b.priority.cmp(&a.priority).then_with(|| a.key.cmp(&b.key)));
        v
    }
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
