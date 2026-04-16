//! Public notification data types.
//!
//! Defines the enums and structs that make up the notification domain:
//! agent state, notification source, flash reason, workspace status,
//! notification payload, and per-pane notification state.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

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
///
/// The key is carried on the `BTreeMap` — it's not duplicated here. Iterate
/// via [`WorkspaceStatus::entries_by_priority`] to get `(&str, &StatusEntry)`
/// pairs.
#[derive(Debug, Clone)]
pub struct StatusEntry {
    pub text: String,
    pub priority: i32,
    pub icon: Option<String>,
    pub color: Option<[u8; 4]>,
    pub updated_at: Instant,
    /// Absolute deadline after which the entry is considered expired and is
    /// filtered out of the render list. `None` means there is no automatic
    /// expiry — the entry persists until removed via
    /// [`super::NotificationStore::remove_entry`] or overwritten by a later
    /// [`super::NotificationStore::upsert_entry`] call for the same key.
    ///
    /// TTL acts as a safety net for integrations that publish a transient
    /// entry (e.g. "running tool X") but might not survive to clean it up
    /// (crashed hook, killed subprocess). Set to `None` for legacy sidebar
    /// slots — those are owned by `set_status` and live until overwritten.
    pub expires_at: Option<Instant>,
}

impl StatusEntry {
    /// True if this entry's TTL has passed at `now`. Sticky (no-TTL)
    /// entries always return false.
    pub fn is_expired(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|t| now >= t)
    }

    /// Convert a duration into an absolute `expires_at` anchored at `now`.
    /// Helper for callers composing their own `StatusEntry` values.
    ///
    /// Uses `checked_add` so that a pathologically large `ttl` (e.g. a
    /// malicious IPC client sending `ttl_ms = u64::MAX`) collapses to
    /// `None` — semantically equivalent to "sticky, no expiry" — instead
    /// of panicking on `Instant` overflow.
    pub fn ttl_to_expires_at(now: Instant, ttl: Option<Duration>) -> Option<Instant> {
        ttl.and_then(|d| now.checked_add(d))
    }
}

/// Per-workspace agent status displayed as a pill in the sidebar.
///
/// Stores agent state + progress as first-class meta fields, and a keyed
/// dictionary of [`StatusEntry`] rows. Legacy label/task/message are now
/// looked up through the reserved keys in [`priority`]; see
/// [`Self::label`], [`Self::task`], [`Self::message`].
///
/// ## Two-layer state (G3)
///
/// The authoritative state is `entries`: every write (`upsert_entry`,
/// `remove_entry`, `set_status`) updates it immediately. A parallel
/// `displayed` dictionary is the debounced projection the sidebar renders
/// from — it trails `entries` by
/// [`super::NotificationStore::DEBOUNCE_WINDOW`] (40ms) so a burst of rapid
/// tool-call writes doesn't flash through intermediate values the user
/// can't read. `displayed` is refreshed explicitly via
/// [`super::NotificationStore::commit_displayed_at`], typically once per
/// frame.
///
/// `pending_removals` carries keys that were dropped from `entries` but
/// haven't aged out of `displayed` yet. Once `now - removed_at >=
/// DEBOUNCE_WINDOW`, the commit pass drops them from both.
#[derive(Debug, Clone)]
pub struct WorkspaceStatus {
    pub state: AgentState,
    pub updated_at: Instant,
    /// Optional progress value (0.0–1.0) for progress bar display.
    pub progress: Option<f32>,
    /// Optional short label rendered alongside the progress bar (e.g.
    /// `"compiling 34/120"`). Only meaningful when [`Self::progress`] is
    /// `Some` — cleared together with it whenever the bar clears.
    pub progress_label: Option<String>,
    /// Keyed status entries — authoritative, written immediately by
    /// `upsert_entry` / `set_status` / `remove_entry`. Use
    /// [`Self::entries_by_priority`] for the ordered render list;
    /// [`Self::label`] / [`Self::task`] / [`Self::message`] for the three
    /// legacy sidebar slots.
    pub entries: BTreeMap<String, StatusEntry>,
    /// Debounced snapshot of `entries` for rendering. See the type-level
    /// docs; read via [`Self::displayed_by_priority`] /
    /// [`Self::displayed_label`] / [`Self::displayed_task`] /
    /// [`Self::displayed_message`].
    pub displayed: BTreeMap<String, StatusEntry>,
    /// Keys removed from `entries` that still linger in `displayed`
    /// awaiting the debounce window to drop them. Maps to the removal
    /// instant. Empty for most workspaces most of the time.
    pub pending_removals: BTreeMap<String, Instant>,
}

/// Reserved entry keys used by the legacy sidebar view. Named here rather
/// than inline so renames are a single edit.
///
/// The `"agent.*"` namespace is reserved for these three keys and is the only
/// thing that [`NotificationStore::set_status`] writes. Integrations should
/// pick their own namespace (`"claude.tool"`, `"gemini.state"`, …);
/// [`NotificationStore::upsert_entry`] rejects writes whose key begins with
/// `"agent."` so third-party publishers can't stomp the legacy sidebar slots.
///
/// [`NotificationStore::set_status`]: super::NotificationStore::set_status
/// [`NotificationStore::upsert_entry`]: super::NotificationStore::upsert_entry
pub const KEY_AGENT_LABEL: &str = "agent.label";
pub const KEY_AGENT_TASK: &str = "agent.task";
pub const KEY_AGENT_MESSAGE: &str = "agent.message";

/// Reserved key namespace. See [`KEY_AGENT_LABEL`].
pub const AGENT_KEY_PREFIX: &str = "agent.";

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

    /// Non-expired entries sorted by descending priority, then by key for
    /// stable output on ties. Expired entries are filtered out silently —
    /// call [`super::NotificationStore::prune_expired_entries`] periodically
    /// to reclaim their memory. The sidebar will iterate this once G20 lands.
    pub fn entries_by_priority(&self) -> Vec<(&str, &StatusEntry)> {
        self.entries_by_priority_at(Instant::now())
    }

    /// Same as [`Self::entries_by_priority`] but takes the current time
    /// explicitly — used by tests that need deterministic TTL behaviour.
    pub fn entries_by_priority_at(&self, now: Instant) -> Vec<(&str, &StatusEntry)> {
        Self::sorted_by_priority(&self.entries, now)
    }

    // ---- Debounced / displayed-snapshot accessors (G3) --------------------

    /// Debounced label — the displayed projection of [`KEY_AGENT_LABEL`].
    pub fn displayed_label(&self) -> Option<&str> {
        self.displayed.get(KEY_AGENT_LABEL).map(|e| e.text.as_str())
    }

    /// Debounced task.
    pub fn displayed_task(&self) -> Option<&str> {
        self.displayed.get(KEY_AGENT_TASK).map(|e| e.text.as_str())
    }

    /// Debounced message. This is the field the sidebar currently renders
    /// as the per-workspace subtitle; using the displayed projection here
    /// is what actually eliminates tool-boundary flicker for the user.
    pub fn displayed_message(&self) -> Option<&str> {
        self.displayed
            .get(KEY_AGENT_MESSAGE)
            .map(|e| e.text.as_str())
    }

    /// Priority-sorted list of entries from the debounced `displayed`
    /// snapshot. Expired entries (those with `expires_at` in the past) are
    /// filtered out, same as [`Self::entries_by_priority`].
    pub fn displayed_by_priority(&self) -> Vec<(&str, &StatusEntry)> {
        self.displayed_by_priority_at(Instant::now())
    }

    /// Same as [`Self::displayed_by_priority`] but takes the current time
    /// explicitly.
    pub fn displayed_by_priority_at(&self, now: Instant) -> Vec<(&str, &StatusEntry)> {
        Self::sorted_by_priority(&self.displayed, now)
    }

    fn sorted_by_priority(
        src: &BTreeMap<String, StatusEntry>,
        now: Instant,
    ) -> Vec<(&str, &StatusEntry)> {
        let mut v: Vec<(&str, &StatusEntry)> = src
            .iter()
            .filter(|(_, e)| !e.is_expired(now))
            .map(|(k, e)| (k.as_str(), e))
            .collect();
        v.sort_by(|a, b| b.1.priority.cmp(&a.1.priority).then_with(|| a.0.cmp(b.0)));
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
