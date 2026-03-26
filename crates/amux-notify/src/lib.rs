use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------

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

/// Why a pane is flashing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashReason {
    /// Pane received focus via keyboard nav or click (teal).
    Navigation,
    /// A notification just arrived (blue).
    NotificationArrival,
    /// A notification was dismissed (blue).
    NotificationDismiss,
}

// ---------------------------------------------------------------------------
// Structs
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Flash animation constants (matching cmux)
// ---------------------------------------------------------------------------

/// Total duration of the double-pulse flash animation.
pub const FLASH_DURATION: f32 = 0.9;

/// Keyframe times as fractions of FLASH_DURATION.
const FLASH_KEY_TIMES: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

/// Opacity values at each keyframe.
const FLASH_VALUES: [f32; 5] = [0.0, 1.0, 0.0, 1.0, 0.0];

/// Compute flash opacity for the double-pulse pattern at time `t` seconds.
/// Returns 0.0 when `t >= FLASH_DURATION`.
pub fn flash_alpha(t: f32) -> f32 {
    if !(0.0..FLASH_DURATION).contains(&t) {
        return 0.0;
    }
    let frac = t / FLASH_DURATION;
    // Find which segment we're in
    for i in 0..FLASH_KEY_TIMES.len() - 1 {
        let t0 = FLASH_KEY_TIMES[i];
        let t1 = FLASH_KEY_TIMES[i + 1];
        if frac >= t0 && frac < t1 {
            let local = (frac - t0) / (t1 - t0);
            // Alternate easeOut / easeIn per cmux pattern
            let eased = if i % 2 == 0 {
                ease_out(local)
            } else {
                ease_in(local)
            };
            let v0 = FLASH_VALUES[i];
            let v1 = FLASH_VALUES[i + 1];
            return v0 + (v1 - v0) * eased;
        }
    }
    0.0
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t) * (1.0 - t)
}

fn ease_in(t: f32) -> f32 {
    t * t
}

// ---------------------------------------------------------------------------
// NotificationStore
// ---------------------------------------------------------------------------

/// Central store for notifications and agent status. Owned by AmuxApp.
pub struct NotificationStore {
    notifications: Vec<Notification>,
    next_id: u64,
    pane_states: HashMap<u64, PaneNotifyState>,
    workspace_statuses: HashMap<u64, WorkspaceStatus>,
}

impl NotificationStore {
    pub fn new() -> Self {
        Self {
            notifications: Vec::new(),
            next_id: 1,
            pane_states: HashMap::new(),
            workspace_statuses: HashMap::new(),
        }
    }

    /// Add a notification. Triggers a flash on the target pane.
    pub fn push(
        &mut self,
        workspace_id: u64,
        pane_id: u64,
        surface_id: u64,
        title: String,
        body: String,
        source: NotificationSource,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        self.notifications.push(Notification {
            id,
            workspace_id,
            pane_id,
            surface_id,
            title,
            body,
            source,
            created_at: Instant::now(),
            read: false,
        });

        let state = self.pane_states.entry(pane_id).or_default();
        state.unread_count += 1;
        state.flash_started_at = Some(Instant::now());
        state.flash_reason = Some(FlashReason::NotificationArrival);

        id
    }

    /// Push a notification but immediately mark it as read (for focused-pane
    /// notifications — still triggers arrival flash but no persistent ring).
    pub fn push_read(
        &mut self,
        workspace_id: u64,
        pane_id: u64,
        surface_id: u64,
        title: String,
        body: String,
        source: NotificationSource,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        self.notifications.push(Notification {
            id,
            workspace_id,
            pane_id,
            surface_id,
            title,
            body,
            source,
            created_at: Instant::now(),
            read: true,
        });

        // Flash but don't increment unread
        let state = self.pane_states.entry(pane_id).or_default();
        state.flash_started_at = Some(Instant::now());
        state.flash_reason = Some(FlashReason::NotificationArrival);

        id
    }

    /// Mark all notifications for a pane as read, clear the unread count,
    /// and trigger a dismiss flash.
    pub fn mark_pane_read(&mut self, pane_id: u64) {
        let had_unread = self.pane_unread(pane_id) > 0;
        for n in &mut self.notifications {
            if n.pane_id == pane_id && !n.read {
                n.read = true;
            }
        }
        let state = self.pane_states.entry(pane_id).or_default();
        state.unread_count = 0;
        if had_unread {
            state.flash_started_at = Some(Instant::now());
            state.flash_reason = Some(FlashReason::NotificationDismiss);
        }
    }

    /// Mark all notifications for a workspace as read.
    pub fn mark_workspace_read(&mut self, pane_ids: &[u64]) {
        let pane_set: HashSet<u64> = pane_ids.iter().copied().collect();
        for n in &mut self.notifications {
            if !n.read && pane_set.contains(&n.pane_id) {
                n.read = true;
            }
        }
        for &pid in pane_ids {
            if let Some(state) = self.pane_states.get_mut(&pid) {
                if state.unread_count > 0 {
                    state.unread_count = 0;
                    state.flash_started_at = Some(Instant::now());
                    state.flash_reason = Some(FlashReason::NotificationDismiss);
                }
            }
        }
    }

    /// Mark all notifications as read.
    pub fn mark_all_read(&mut self) {
        for n in &mut self.notifications {
            n.read = true;
        }
        for state in self.pane_states.values_mut() {
            if state.unread_count > 0 {
                state.unread_count = 0;
                state.flash_started_at = Some(Instant::now());
                state.flash_reason = Some(FlashReason::NotificationDismiss);
            }
        }
    }

    /// Remove a specific notification by id.
    pub fn remove_notification(&mut self, notification_id: u64) {
        if let Some(pos) = self
            .notifications
            .iter()
            .position(|n| n.id == notification_id)
        {
            let notif = self.notifications.remove(pos);
            if !notif.read {
                if let Some(state) = self.pane_states.get_mut(&notif.pane_id) {
                    state.unread_count = state.unread_count.saturating_sub(1);
                }
            }
        }
    }

    /// Clear all notifications.
    pub fn clear_all(&mut self) {
        self.notifications.clear();
        for state in self.pane_states.values_mut() {
            state.unread_count = 0;
        }
    }

    /// Trigger a flash on a pane without adding a notification.
    pub fn flash_pane(&mut self, pane_id: u64, reason: FlashReason) {
        let state = self.pane_states.entry(pane_id).or_default();
        state.flash_started_at = Some(Instant::now());
        state.flash_reason = Some(reason);
    }

    /// Get unread count for a pane.
    pub fn pane_unread(&self, pane_id: u64) -> usize {
        self.pane_states.get(&pane_id).map_or(0, |s| s.unread_count)
    }

    /// Check if any pane in the given set has unread notifications,
    /// excluding the specified focused pane.
    pub fn has_unread_excluding(&self, pane_ids: &[u64], focused_pane_id: u64) -> bool {
        pane_ids.iter().any(|&id| {
            id != focused_pane_id
                && self
                    .pane_states
                    .get(&id)
                    .is_some_and(|s| s.unread_count > 0)
        })
    }

    /// Total unread count across the given pane set.
    pub fn workspace_unread_count(&self, pane_ids: &[u64]) -> usize {
        pane_ids.iter().map(|id| self.pane_unread(*id)).sum()
    }

    /// Get pane visual state (for ring + flash rendering).
    pub fn pane_state(&self, pane_id: u64) -> Option<&PaneNotifyState> {
        self.pane_states.get(&pane_id)
    }

    /// All notifications, oldest first.
    pub fn all_notifications(&self) -> &[Notification] {
        &self.notifications
    }

    /// Find the most recent unread notification.
    pub fn most_recent_unread(&self) -> Option<&Notification> {
        self.notifications.iter().rev().find(|n| !n.read)
    }

    /// Find the most recent notification for a workspace (read or unread).
    pub fn latest_for_workspace(&self, workspace_id: u64) -> Option<&Notification> {
        self.notifications
            .iter()
            .rev()
            .find(|n| n.workspace_id == workspace_id)
    }

    /// Set workspace agent status. Clears any existing progress bar.
    pub fn set_status(
        &mut self,
        workspace_id: u64,
        state: AgentState,
        label: Option<String>,
        task: Option<String>,
        message: Option<String>,
    ) {
        let existing = self.workspace_statuses.get(&workspace_id);
        // Normalize empty strings to None, then preserve existing if not provided.
        // Some("") means "clear", None means "keep previous".
        let task = match task {
            Some(s) if s.is_empty() => None,
            Some(s) => Some(s),
            None => existing.and_then(|s| s.task.clone()),
        };
        let message = match message {
            Some(s) if s.is_empty() => None,
            Some(s) => Some(s),
            None => existing.and_then(|s| s.message.clone()),
        };
        self.workspace_statuses.insert(
            workspace_id,
            WorkspaceStatus {
                state,
                label,
                updated_at: Instant::now(),
                progress: None,
                task,
                message,
            },
        );
    }

    /// Set workspace progress (0.0–1.0). Pass `None` to clear.
    pub fn set_progress(&mut self, workspace_id: u64, progress: Option<f32>) {
        if let Some(status) = self.workspace_statuses.get_mut(&workspace_id) {
            status.progress = progress;
            status.updated_at = Instant::now();
        }
    }

    /// Get workspace agent status.
    pub fn workspace_status(&self, workspace_id: u64) -> Option<&WorkspaceStatus> {
        self.workspace_statuses.get(&workspace_id)
    }

    /// Clean up all state for a closed pane.
    pub fn remove_pane(&mut self, pane_id: u64) {
        self.pane_states.remove(&pane_id);
        self.notifications.retain(|n| n.pane_id != pane_id);
    }

    /// Clean up all state for a closed workspace.
    pub fn remove_workspace(&mut self, workspace_id: u64) {
        self.workspace_statuses.remove(&workspace_id);
        // Collect pane IDs to remove
        let pane_ids: Vec<u64> = self
            .notifications
            .iter()
            .filter(|n| n.workspace_id == workspace_id)
            .map(|n| n.pane_id)
            .collect();
        for pid in &pane_ids {
            self.pane_states.remove(pid);
        }
        self.notifications
            .retain(|n| n.workspace_id != workspace_id);
    }
}

impl Default for NotificationStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_increments_unread() {
        let mut store = NotificationStore::new();
        let id = store.push(
            1,
            10,
            100,
            "Test".into(),
            "body".into(),
            NotificationSource::Bell,
        );
        assert_eq!(id, 1);
        assert_eq!(store.pane_unread(10), 1);
        assert_eq!(store.all_notifications().len(), 1);
        assert!(!store.all_notifications()[0].read);
    }

    #[test]
    fn push_read_does_not_increment_unread() {
        let mut store = NotificationStore::new();
        store.push_read(
            1,
            10,
            100,
            "Test".into(),
            "body".into(),
            NotificationSource::Toast,
        );
        assert_eq!(store.pane_unread(10), 0);
        assert_eq!(store.all_notifications().len(), 1);
        assert!(store.all_notifications()[0].read);
        // But flash should still be set
        assert!(store.pane_state(10).unwrap().flash_started_at.is_some());
    }

    #[test]
    fn mark_pane_read_clears_unread() {
        let mut store = NotificationStore::new();
        store.push(1, 10, 100, "A".into(), "a".into(), NotificationSource::Bell);
        store.push(
            1,
            10,
            101,
            "B".into(),
            "b".into(),
            NotificationSource::Toast,
        );
        assert_eq!(store.pane_unread(10), 2);

        store.mark_pane_read(10);
        assert_eq!(store.pane_unread(10), 0);
        assert!(store.all_notifications().iter().all(|n| n.read));
        // Dismiss flash triggered
        assert_eq!(
            store.pane_state(10).unwrap().flash_reason,
            Some(FlashReason::NotificationDismiss)
        );
    }

    #[test]
    fn mark_all_read() {
        let mut store = NotificationStore::new();
        store.push(1, 10, 100, "A".into(), "a".into(), NotificationSource::Bell);
        store.push(2, 20, 200, "B".into(), "b".into(), NotificationSource::Cli);
        store.mark_all_read();
        assert_eq!(store.pane_unread(10), 0);
        assert_eq!(store.pane_unread(20), 0);
    }

    #[test]
    fn most_recent_unread() {
        let mut store = NotificationStore::new();
        store.push(
            1,
            10,
            100,
            "First".into(),
            "a".into(),
            NotificationSource::Bell,
        );
        store.push(
            1,
            20,
            200,
            "Second".into(),
            "b".into(),
            NotificationSource::Toast,
        );
        let recent = store.most_recent_unread().unwrap();
        assert_eq!(recent.title, "Second");

        store.mark_pane_read(20);
        let recent = store.most_recent_unread().unwrap();
        assert_eq!(recent.title, "First");

        store.mark_pane_read(10);
        assert!(store.most_recent_unread().is_none());
    }

    #[test]
    fn has_unread_excluding_focused() {
        let mut store = NotificationStore::new();
        store.push(1, 10, 100, "A".into(), "a".into(), NotificationSource::Bell);
        store.push(1, 20, 200, "B".into(), "b".into(), NotificationSource::Bell);

        assert!(store.has_unread_excluding(&[10, 20], 10));
        assert!(store.has_unread_excluding(&[10, 20], 20));
        assert!(!store.has_unread_excluding(&[10], 10));
    }

    #[test]
    fn workspace_unread_count() {
        let mut store = NotificationStore::new();
        store.push(1, 10, 100, "A".into(), "a".into(), NotificationSource::Bell);
        store.push(1, 10, 101, "B".into(), "b".into(), NotificationSource::Bell);
        store.push(
            1,
            20,
            200,
            "C".into(),
            "c".into(),
            NotificationSource::Toast,
        );
        assert_eq!(store.workspace_unread_count(&[10, 20]), 3);
    }

    #[test]
    fn flash_pane_sets_flash() {
        let mut store = NotificationStore::new();
        store.flash_pane(10, FlashReason::Navigation);
        let state = store.pane_state(10).unwrap();
        assert_eq!(state.flash_reason, Some(FlashReason::Navigation));
        assert!(state.flash_started_at.is_some());
        assert_eq!(state.unread_count, 0);
    }

    #[test]
    fn remove_pane_cleanup() {
        let mut store = NotificationStore::new();
        store.push(1, 10, 100, "A".into(), "a".into(), NotificationSource::Bell);
        store.push(1, 20, 200, "B".into(), "b".into(), NotificationSource::Bell);
        store.remove_pane(10);
        assert!(store.pane_state(10).is_none());
        assert_eq!(store.all_notifications().len(), 1);
        assert_eq!(store.all_notifications()[0].pane_id, 20);
    }

    #[test]
    fn remove_notification_decrements_unread() {
        let mut store = NotificationStore::new();
        let id = store.push(1, 10, 100, "A".into(), "a".into(), NotificationSource::Bell);
        assert_eq!(store.pane_unread(10), 1);
        store.remove_notification(id);
        assert_eq!(store.pane_unread(10), 0);
        assert_eq!(store.all_notifications().len(), 0);
    }

    #[test]
    fn workspace_status_roundtrip() {
        let mut store = NotificationStore::new();
        store.set_status(
            1,
            AgentState::Active,
            Some("Running tests".into()),
            None,
            None,
        );
        let status = store.workspace_status(1).unwrap();
        assert_eq!(status.state, AgentState::Active);
        assert_eq!(status.label.as_deref(), Some("Running tests"));

        store.set_status(1, AgentState::Idle, None, None, None);
        let status = store.workspace_status(1).unwrap();
        assert_eq!(status.state, AgentState::Idle);
        assert!(status.label.is_none());
    }

    #[test]
    fn flash_alpha_pattern() {
        // Start: 0
        assert_eq!(flash_alpha(0.0), 0.0);
        // Peak 1: around 0.225s
        let a1 = flash_alpha(0.225);
        assert!(a1 > 0.9, "first peak should be near 1.0, got {a1}");
        // Trough: around 0.45s
        let a2 = flash_alpha(0.45);
        assert!(a2 < 0.1, "trough should be near 0.0, got {a2}");
        // Peak 2: around 0.675s
        let a3 = flash_alpha(0.675);
        assert!(a3 > 0.9, "second peak should be near 1.0, got {a3}");
        // End: 0
        assert_eq!(flash_alpha(0.9), 0.0);
        // Past end: 0
        assert_eq!(flash_alpha(1.0), 0.0);
    }

    #[test]
    fn clear_all_notifications() {
        let mut store = NotificationStore::new();
        store.push(1, 10, 100, "A".into(), "a".into(), NotificationSource::Bell);
        store.push(2, 20, 200, "B".into(), "b".into(), NotificationSource::Cli);
        store.clear_all();
        assert_eq!(store.all_notifications().len(), 0);
        assert_eq!(store.pane_unread(10), 0);
        assert_eq!(store.pane_unread(20), 0);
    }

    #[test]
    fn mark_workspace_read_only_affects_given_panes() {
        let mut store = NotificationStore::new();
        // Workspace 1 panes: 10, 11
        store.push(1, 10, 100, "A".into(), "a".into(), NotificationSource::Bell);
        store.push(1, 11, 101, "B".into(), "b".into(), NotificationSource::Bell);
        // Workspace 2 pane: 20
        store.push(2, 20, 200, "C".into(), "c".into(), NotificationSource::Bell);

        store.mark_workspace_read(&[10, 11]);
        assert_eq!(store.pane_unread(10), 0);
        assert_eq!(store.pane_unread(11), 0);
        assert_eq!(store.pane_unread(20), 1); // unaffected
    }

    #[test]
    fn latest_for_workspace_returns_most_recent() {
        let mut store = NotificationStore::new();
        store.push(
            1,
            10,
            100,
            "First".into(),
            "a".into(),
            NotificationSource::Bell,
        );
        store.push(
            2,
            20,
            200,
            "Other".into(),
            "b".into(),
            NotificationSource::Bell,
        );
        store.push(
            1,
            10,
            101,
            "Latest".into(),
            "c".into(),
            NotificationSource::Bell,
        );

        let latest = store.latest_for_workspace(1).unwrap();
        assert_eq!(latest.title, "Latest");

        let latest2 = store.latest_for_workspace(2).unwrap();
        assert_eq!(latest2.title, "Other");

        assert!(store.latest_for_workspace(99).is_none());
    }

    #[test]
    fn progress_lifecycle() {
        let mut store = NotificationStore::new();
        // set_status creates entry with no progress
        store.set_status(1, AgentState::Active, Some("Building".into()), None, None);
        assert!(store.workspace_status(1).unwrap().progress.is_none());

        // set_progress adds progress
        store.set_progress(1, Some(0.5));
        assert_eq!(store.workspace_status(1).unwrap().progress, Some(0.5));

        // set_status clears progress
        store.set_status(1, AgentState::Idle, None, None, None);
        assert!(store.workspace_status(1).unwrap().progress.is_none());
    }
}
