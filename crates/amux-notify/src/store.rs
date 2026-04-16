//! Central notification store and agent status management.
//!
//! `NotificationStore` is the single source of truth for all notifications
//! and per-workspace agent status. It handles notification push/supersession,
//! unread counting, workspace status updates, flash animation triggering,
//! and per-pane notification state.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::types::*;

/// Helper for the [`NotificationStore::set_status`] back-compat shim.
///
/// Interprets the legacy `Some("")` / `Some(value)` / `None` convention:
/// empty string expires the entry, non-empty upserts it, `None` leaves the
/// existing entry untouched. Operates on both `entries` (authoritative)
/// and `pending_removals` (G3 debounce tombstones) so a removal survives
/// the debounce window and an upsert cancels any prior pending removal
/// for the same key.
fn apply_legacy_field(
    entries: &mut BTreeMap<String, StatusEntry>,
    pending_removals: &mut BTreeMap<String, Instant>,
    key: &str,
    priority: i32,
    value: Option<String>,
    now: Instant,
) {
    match value {
        None => {}
        Some(s) if s.is_empty() && entries.remove(key).is_some() => {
            pending_removals.insert(key.to_string(), now);
        }
        Some(s) if s.is_empty() => {}
        Some(s) => {
            entries.insert(
                key.to_string(),
                StatusEntry {
                    text: s,
                    priority,
                    icon: None,
                    color: None,
                    updated_at: now,
                    // Legacy sidebar slots are sticky: they live until the
                    // next set_status overwrites or clears them. TTL is only
                    // ever attached by upsert_entry callers.
                    expires_at: None,
                },
            );
            // Re-insertion cancels any pending removal for the same key —
            // otherwise a later `commit_displayed_at` could drop the
            // just-re-inserted entry once the old tombstone window elapses.
            pending_removals.remove(key);
        }
    }
}

/// Returns `true` when two entries carry the same user-visible value —
/// ignores `updated_at` (commit timing, not content) and `expires_at`
/// (a TTL bump alone shouldn't trigger a re-render).
fn status_entry_eq(a: &StatusEntry, b: &StatusEntry) -> bool {
    a.text == b.text && a.priority == b.priority && a.icon == b.icon && a.color == b.color
}

/// Apply G3 debounce to a single workspace's status in-place. Shared by
/// [`NotificationStore::commit_displayed_at`] and the per-workspace test
/// entry point. Returns `true` if anything about `displayed` actually
/// changed (insert, update, or remove) so callers can track whether a
/// repaint is warranted.
fn commit_workspace_displayed(
    status: &mut WorkspaceStatus,
    now: Instant,
    debounce: Duration,
) -> bool {
    let mut changed = false;
    // Promote writes that have been stable for at least `debounce`.
    for (key, entry) in &status.entries {
        if now.saturating_duration_since(entry.updated_at) < debounce {
            continue;
        }
        let existing = status.displayed.get(key);
        if existing.is_some_and(|d| status_entry_eq(d, entry)) {
            continue;
        }
        status.displayed.insert(key.clone(), entry.clone());
        changed = true;
    }
    // Drop tombstones whose debounce window has expired.
    let ready: Vec<String> = status
        .pending_removals
        .iter()
        .filter(|(_, &t)| now.saturating_duration_since(t) >= debounce)
        .map(|(k, _)| k.clone())
        .collect();
    for key in ready {
        status.pending_removals.remove(&key);
        // Only mark `changed` when `displayed` actually loses something.
        if status.displayed.remove(&key).is_some() {
            changed = true;
        }
    }
    // Defensive cleanup: a key that vanished from `entries` without a
    // tombstone (possible if `entries` was mutated directly in tests)
    // shouldn't linger in `displayed` forever. Only purges after a full
    // debounce window to keep the semantics of removals consistent.
    let orphans: Vec<String> = status
        .displayed
        .iter()
        .filter(|(k, _)| {
            !status.entries.contains_key(*k) && !status.pending_removals.contains_key(*k)
        })
        .map(|(k, _)| k.clone())
        .collect();
    for key in orphans {
        status.displayed.remove(&key);
        changed = true;
    }
    changed
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
    /// Default debounce window for the `displayed` snapshot promotion
    /// pass (parity plan G3). Matches cmux's 40ms trailing debounce —
    /// the interval is small enough that intentional status changes still
    /// feel instant, but long enough that a burst of tool-call writes
    /// doesn't flash through values the user can't read.
    pub const DEBOUNCE_WINDOW: Duration = Duration::from_millis(40);

    pub fn new() -> Self {
        Self {
            notifications: Vec::new(),
            next_id: 1,
            pane_states: HashMap::new(),
            workspace_statuses: HashMap::new(),
        }
    }

    /// Remove prior **unread** notifications for the given (workspace, surface)
    /// pair so only the newest notification for that surface remains active.
    /// Read notifications are preserved as historical records and are never
    /// dropped by this helper. Unread counts for the affected panes are
    /// adjusted accordingly. Matches cmux's "only most recent notification per
    /// tab+surface matters" model, preventing notification pile-up during a
    /// single agent session.
    fn supersede_prior_for_surface(&mut self, workspace_id: u64, surface_id: u64) {
        let mut removed_unread_by_pane: HashMap<u64, usize> = HashMap::new();
        self.notifications.retain(|n| {
            if n.workspace_id == workspace_id && n.surface_id == surface_id && !n.read {
                *removed_unread_by_pane.entry(n.pane_id).or_insert(0) += 1;
                false
            } else {
                true
            }
        });
        for (pid, count) in removed_unread_by_pane {
            if let Some(state) = self.pane_states.get_mut(&pid) {
                state.unread_count = state.unread_count.saturating_sub(count);
            }
        }
    }

    /// Add a notification. Triggers a flash on the target pane.
    /// Supersedes any existing **unread** notifications for the same
    /// (workspace, surface); read notifications are retained as history.
    #[allow(clippy::too_many_arguments)]
    pub fn push(
        &mut self,
        workspace_id: u64,
        pane_id: u64,
        surface_id: u64,
        title: String,
        subtitle: String,
        body: String,
        source: NotificationSource,
    ) -> u64 {
        self.supersede_prior_for_surface(workspace_id, surface_id);

        let id = self.next_id;
        self.next_id += 1;

        self.notifications.push(Notification {
            id,
            workspace_id,
            pane_id,
            surface_id,
            title,
            subtitle,
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
    /// Supersedes any existing **unread** notifications for the same
    /// (workspace, surface); read notifications are retained as history.
    #[allow(clippy::too_many_arguments)]
    pub fn push_read(
        &mut self,
        workspace_id: u64,
        pane_id: u64,
        surface_id: u64,
        title: String,
        subtitle: String,
        body: String,
        source: NotificationSource,
    ) -> u64 {
        self.supersede_prior_for_surface(workspace_id, surface_id);

        let id = self.next_id;
        self.next_id += 1;

        self.notifications.push(Notification {
            id,
            workspace_id,
            pane_id,
            surface_id,
            title,
            subtitle,
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

    /// Restore a historical read notification from a saved session without
    /// triggering supersession or a flash. Used only during session restore;
    /// preserves chronological history that would otherwise be collapsed by
    /// the per-(workspace, surface) supersession in [`Self::push_read`].
    #[allow(clippy::too_many_arguments)]
    pub fn push_restored(
        &mut self,
        workspace_id: u64,
        pane_id: u64,
        surface_id: u64,
        title: String,
        subtitle: String,
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
            subtitle,
            body,
            source,
            created_at: Instant::now(),
            read: true,
        });

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

    /// Total unread count across all panes.
    pub fn total_unread(&self) -> usize {
        self.pane_states.values().map(|s| s.unread_count).sum()
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
            .find(|n| n.workspace_id == workspace_id && !n.read)
    }

    /// Set workspace agent status. Clears any existing progress bar.
    ///
    /// Back-compat shim over the keyed-entry model: `label`/`task`/`message`
    /// map to the reserved [`KEY_AGENT_LABEL`] / [`KEY_AGENT_TASK`] /
    /// [`KEY_AGENT_MESSAGE`] keys. Preserves the historical convention where
    /// `Some("")` expires the entry and `None` leaves it untouched, so
    /// callers driven by the `status.set` IPC can continue to publish just
    /// the fields they want to change.
    ///
    /// `state` is **always** authoritative and replaces the existing agent
    /// state on every call — the `None`-preserves convention applies only to
    /// the three text fields handled via the shim. Callers that want to
    /// publish text under their own key without mutating agent state should
    /// use [`Self::upsert_entry`] instead.
    pub fn set_status(
        &mut self,
        workspace_id: u64,
        state: AgentState,
        label: Option<String>,
        task: Option<String>,
        message: Option<String>,
    ) {
        self.set_status_at(workspace_id, state, label, task, message, Instant::now());
    }

    /// Same as [`Self::set_status`] but takes the current time explicitly —
    /// used by tests that need deterministic debounce behaviour without
    /// racing `Instant::now()` inside the store.
    pub fn set_status_at(
        &mut self,
        workspace_id: u64,
        state: AgentState,
        label: Option<String>,
        task: Option<String>,
        message: Option<String>,
        now: Instant,
    ) {
        let entry = self
            .workspace_statuses
            .entry(workspace_id)
            .or_insert_with(|| WorkspaceStatus {
                state,
                updated_at: now,
                progress: None,
                progress_label: None,
                entries: BTreeMap::new(),
                displayed: BTreeMap::new(),
                pending_removals: BTreeMap::new(),
            });
        entry.state = state;
        entry.updated_at = now;
        // set_status is a "coarse" update: clears any existing progress.
        entry.progress = None;
        entry.progress_label = None;

        apply_legacy_field(
            &mut entry.entries,
            &mut entry.pending_removals,
            KEY_AGENT_LABEL,
            priority::LABEL,
            label,
            now,
        );
        apply_legacy_field(
            &mut entry.entries,
            &mut entry.pending_removals,
            KEY_AGENT_TASK,
            priority::TASK,
            task,
            now,
        );
        apply_legacy_field(
            &mut entry.entries,
            &mut entry.pending_removals,
            KEY_AGENT_MESSAGE,
            priority::MESSAGE,
            message,
            now,
        );
    }

    /// Publish / replace a keyed status entry for a workspace.
    ///
    /// Primary API for [#260](https://github.com/daveowenatl/amux/issues/260)
    /// parity work: hooks, CLI, and integrations publish under their own key
    /// so that one source expiring (e.g. a tool completing) doesn't blank
    /// another source's content. Creates the workspace-status record with
    /// [`AgentState::Idle`] if it didn't already exist — callers that need a
    /// specific state should also call [`Self::set_status`].
    ///
    /// Keys beginning with [`AGENT_KEY_PREFIX`] (`"agent."`) are reserved for
    /// the legacy sidebar slots written by [`Self::set_status`] and are
    /// rejected here with a warning log; use `set_status` to write those.
    // TODO(parity/followup): collapse (icon, color, ttl) into a small
    // `EntryOptions { .. }` builder struct to tame the arg count. Kept as
    // positional for now so G1→G2 stays a minimal additive change.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_entry(
        &mut self,
        workspace_id: u64,
        key: impl Into<String>,
        text: impl Into<String>,
        priority: i32,
        icon: Option<String>,
        color: Option<[u8; 4]>,
        ttl: Option<Duration>,
    ) {
        self.upsert_entry_at(
            workspace_id,
            key,
            text,
            priority,
            icon,
            color,
            ttl,
            Instant::now(),
        );
    }

    /// Same as [`Self::upsert_entry`] but takes the current time explicitly —
    /// used by tests that need deterministic debounce behaviour.
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_entry_at(
        &mut self,
        workspace_id: u64,
        key: impl Into<String>,
        text: impl Into<String>,
        priority: i32,
        icon: Option<String>,
        color: Option<[u8; 4]>,
        ttl: Option<Duration>,
        now: Instant,
    ) {
        let key = key.into();
        if key.starts_with(AGENT_KEY_PREFIX) {
            tracing::warn!(
                "upsert_entry rejected reserved key '{key}' (use set_status for agent.* slots)"
            );
            return;
        }
        let text = text.into();
        let expires_at = StatusEntry::ttl_to_expires_at(now, ttl);
        let status = self
            .workspace_statuses
            .entry(workspace_id)
            .or_insert_with(|| WorkspaceStatus {
                state: AgentState::Idle,
                updated_at: now,
                progress: None,
                progress_label: None,
                entries: BTreeMap::new(),
                displayed: BTreeMap::new(),
                pending_removals: BTreeMap::new(),
            });
        status.updated_at = now;
        // Re-insertion supersedes any tombstone for the same key — see
        // the commentary on `apply_legacy_field` for why this matters.
        status.pending_removals.remove(&key);
        status.entries.insert(
            key,
            StatusEntry {
                text,
                priority,
                icon,
                color,
                updated_at: now,
                expires_at,
            },
        );
    }

    /// Drop expired entries from a workspace's status. Returns the number of
    /// entries removed — callers can use this to decide whether a redraw is
    /// warranted.
    ///
    /// Safe to call on any interval; does nothing if the workspace has no
    /// expired entries (or doesn't exist). A cheap opportunistic sweep is
    /// enough — expired entries are already filtered out of
    /// [`WorkspaceStatus::entries_by_priority`] at render time, so the only
    /// cost of *not* pruning is the memory footprint of a dangling entry.
    pub fn prune_expired_entries(&mut self, workspace_id: u64) -> usize {
        self.prune_expired_entries_at(workspace_id, Instant::now())
    }

    /// Same as [`Self::prune_expired_entries`] but takes the current time
    /// explicitly — used by tests that need deterministic TTL behaviour
    /// without `std::thread::sleep`.
    pub fn prune_expired_entries_at(&mut self, workspace_id: u64, now: Instant) -> usize {
        let Some(status) = self.workspace_statuses.get_mut(&workspace_id) else {
            return 0;
        };
        let before = status.entries.len();
        status.entries.retain(|_, e| !e.is_expired(now));
        let removed = before - status.entries.len();
        if removed > 0 {
            status.updated_at = now;
        }
        removed
    }

    /// Remove a keyed status entry. Returns `true` if an entry was removed.
    ///
    /// Used by tool-end hooks (G2) to expire just the tool's entry without
    /// disturbing other publishers. Safe to call on a missing workspace.
    ///
    /// G3: if the removed key was already visible in `displayed`, a
    /// tombstone is recorded on `pending_removals` so the displayed
    /// snapshot holds the value for the debounce window before dropping
    /// it — avoids flashing the sidebar empty on a rapid tool-end.
    pub fn remove_entry(&mut self, workspace_id: u64, key: &str) -> bool {
        self.remove_entry_at(workspace_id, key, Instant::now())
    }

    /// Same as [`Self::remove_entry`] but takes the current time
    /// explicitly — used by tests that need deterministic tombstone
    /// behaviour without `std::thread::sleep`.
    pub fn remove_entry_at(&mut self, workspace_id: u64, key: &str, now: Instant) -> bool {
        if let Some(status) = self.workspace_statuses.get_mut(&workspace_id) {
            let removed = status.entries.remove(key).is_some();
            if removed {
                status.updated_at = now;
                // Only tombstone if the key is actually in the displayed
                // snapshot; otherwise the removal has no visible effect
                // and the tombstone would just bloat the map.
                if status.displayed.contains_key(key) {
                    status.pending_removals.insert(key.to_string(), now);
                }
            }
            removed
        } else {
            false
        }
    }

    /// Set workspace progress (0.0–1.0) with an optional short label
    /// (e.g. `"compiling 34/120"`). Pass `progress = None` to clear the
    /// bar entirely; the label is cleared alongside in that case since a
    /// lingering label without a bar has no place to render.
    ///
    /// This is a no-op for workspaces the store has never seen — the
    /// progress bar is a decoration on top of an existing workspace row,
    /// not a reason to allocate one. Call [`Self::set_status`] first if
    /// you need to create the row.
    pub fn set_progress(
        &mut self,
        workspace_id: u64,
        progress: Option<f32>,
        label: Option<String>,
    ) {
        if let Some(status) = self.workspace_statuses.get_mut(&workspace_id) {
            status.progress = progress;
            status.progress_label = if progress.is_some() { label } else { None };
            status.updated_at = Instant::now();
        }
    }

    /// Get workspace agent status.
    pub fn workspace_status(&self, workspace_id: u64) -> Option<&WorkspaceStatus> {
        self.workspace_statuses.get(&workspace_id)
    }

    /// Refresh the `displayed` debounced snapshot across all workspaces.
    /// Call once per frame (or on each event tick) with `Instant::now()`
    /// and [`Self::DEBOUNCE_WINDOW`].
    ///
    /// For each workspace:
    /// - promote `entries[k]` into `displayed[k]` when
    ///   `now - entry.updated_at >= debounce` (the write has been stable
    ///   for long enough to display);
    /// - drop keys from `displayed` when `pending_removals[k] + debounce
    ///   <= now` (a removal has survived the debounce window);
    /// - garbage-collect stale `displayed` entries that are neither in
    ///   `entries` nor tombstoned — this is defensive cleanup and should
    ///   never fire in normal operation.
    ///
    /// Returns the number of workspaces whose displayed state changed —
    /// callers can use this to decide whether to request a repaint.
    pub fn commit_displayed_at(&mut self, now: Instant, debounce: Duration) -> usize {
        let mut changed = 0usize;
        for status in self.workspace_statuses.values_mut() {
            if commit_workspace_displayed(status, now, debounce) {
                changed += 1;
            }
        }
        changed
    }

    /// Earliest `Instant` at which a pending debounce transition will be
    /// ready to apply. Returns `None` if nothing is pending. Callers use
    /// this to wake the frame loop at the right moment rather than
    /// polling blindly — e.g. `ctx.request_repaint_after(wakeup - now)`.
    ///
    /// Only entries that would actually change `displayed` on commit are
    /// counted: an entry whose text already matches `displayed[k]`
    /// contributes nothing (there's no pending work for it).
    pub fn next_commit_at(&self, debounce: Duration) -> Option<Instant> {
        let mut earliest: Option<Instant> = None;
        let mut consider = |deadline: Instant| {
            earliest = Some(match earliest {
                Some(existing) if existing <= deadline => existing,
                _ => deadline,
            });
        };
        for status in self.workspace_statuses.values() {
            for (key, entry) in &status.entries {
                let already_shown = status
                    .displayed
                    .get(key)
                    .is_some_and(|d| status_entry_eq(d, entry));
                if !already_shown {
                    consider(entry.updated_at + debounce);
                }
            }
            for &ts in status.pending_removals.values() {
                consider(ts + debounce);
            }
        }
        earliest
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
    use crate::flash::flash_alpha;

    #[test]
    fn push_increments_unread() {
        let mut store = NotificationStore::new();
        let id = store.push(
            1,
            10,
            100,
            "Test".into(),
            "Permission Required".into(),
            "body".into(),
            NotificationSource::Bell,
        );
        assert_eq!(id, 1);
        assert_eq!(store.pane_unread(10), 1);
        assert_eq!(store.all_notifications().len(), 1);
        assert_eq!(store.all_notifications()[0].subtitle, "Permission Required");
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
            "Task Completed".into(),
            "body".into(),
            NotificationSource::Toast,
        );
        assert_eq!(store.pane_unread(10), 0);
        assert_eq!(store.all_notifications().len(), 1);
        assert_eq!(store.all_notifications()[0].subtitle, "Task Completed");
        assert!(store.all_notifications()[0].read);
        // But flash should still be set
        assert!(store.pane_state(10).unwrap().flash_started_at.is_some());
    }

    #[test]
    fn mark_pane_read_clears_unread() {
        let mut store = NotificationStore::new();
        store.push(
            1,
            10,
            100,
            "A".into(),
            String::new(),
            "a".into(),
            NotificationSource::Bell,
        );
        store.push(
            1,
            10,
            101,
            "B".into(),
            String::new(),
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
        store.push(
            1,
            10,
            100,
            "A".into(),
            String::new(),
            "a".into(),
            NotificationSource::Bell,
        );
        store.push(
            2,
            20,
            200,
            "B".into(),
            String::new(),
            "b".into(),
            NotificationSource::Cli,
        );
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
            String::new(),
            "a".into(),
            NotificationSource::Bell,
        );
        store.push(
            1,
            20,
            200,
            "Second".into(),
            String::new(),
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
        store.push(
            1,
            10,
            100,
            "A".into(),
            String::new(),
            "a".into(),
            NotificationSource::Bell,
        );
        store.push(
            1,
            20,
            200,
            "B".into(),
            String::new(),
            "b".into(),
            NotificationSource::Bell,
        );

        assert!(store.has_unread_excluding(&[10, 20], 10));
        assert!(store.has_unread_excluding(&[10, 20], 20));
        assert!(!store.has_unread_excluding(&[10], 10));
    }

    #[test]
    fn workspace_unread_count() {
        let mut store = NotificationStore::new();
        store.push(
            1,
            10,
            100,
            "A".into(),
            String::new(),
            "a".into(),
            NotificationSource::Bell,
        );
        store.push(
            1,
            10,
            101,
            "B".into(),
            String::new(),
            "b".into(),
            NotificationSource::Bell,
        );
        store.push(
            1,
            20,
            200,
            "C".into(),
            String::new(),
            "c".into(),
            NotificationSource::Toast,
        );
        assert_eq!(store.workspace_unread_count(&[10, 20]), 3);
    }

    #[test]
    fn flash_pane_sets_flash() {
        let mut store = NotificationStore::new();
        store.flash_pane(10, FlashReason::NotificationArrival);
        let state = store.pane_state(10).unwrap();
        assert_eq!(state.flash_reason, Some(FlashReason::NotificationArrival));
        assert!(state.flash_started_at.is_some());
        assert_eq!(state.unread_count, 0);
    }

    #[test]
    fn remove_pane_cleanup() {
        let mut store = NotificationStore::new();
        store.push(
            1,
            10,
            100,
            "A".into(),
            String::new(),
            "a".into(),
            NotificationSource::Bell,
        );
        store.push(
            1,
            20,
            200,
            "B".into(),
            String::new(),
            "b".into(),
            NotificationSource::Bell,
        );
        store.remove_pane(10);
        assert!(store.pane_state(10).is_none());
        assert_eq!(store.all_notifications().len(), 1);
        assert_eq!(store.all_notifications()[0].pane_id, 20);
    }

    #[test]
    fn remove_notification_decrements_unread() {
        let mut store = NotificationStore::new();
        let id = store.push(
            1,
            10,
            100,
            "A".into(),
            String::new(),
            "a".into(),
            NotificationSource::Bell,
        );
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
        assert_eq!(status.label(), Some("Running tests"));

        // None preserves label under the legacy shim.
        store.set_status(1, AgentState::Idle, None, None, None);
        let status = store.workspace_status(1).unwrap();
        assert_eq!(status.state, AgentState::Idle);
        assert_eq!(status.label(), Some("Running tests"));

        // Some("") expires the entry.
        store.set_status(1, AgentState::Idle, Some(String::new()), None, None);
        let status = store.workspace_status(1).unwrap();
        assert!(status.label().is_none());
    }

    #[test]
    fn set_status_maps_to_reserved_keys() {
        let mut store = NotificationStore::new();
        store.set_status(
            1,
            AgentState::Active,
            Some("Working".into()),
            Some("Refactor foo".into()),
            Some("reading bar.rs".into()),
        );
        let status = store.workspace_status(1).unwrap();
        assert_eq!(status.label(), Some("Working"));
        assert_eq!(status.task(), Some("Refactor foo"));
        assert_eq!(status.message(), Some("reading bar.rs"));
        assert_eq!(status.entries.len(), 3);

        // Entries surface with the expected priorities.
        let by_pri = status.entries_by_priority();
        assert_eq!(by_pri[0].0, KEY_AGENT_LABEL);
        assert_eq!(by_pri[1].0, KEY_AGENT_TASK);
        assert_eq!(by_pri[2].0, KEY_AGENT_MESSAGE);
    }

    #[test]
    fn upsert_entry_and_remove_entry() {
        let mut store = NotificationStore::new();
        store.upsert_entry(
            1,
            "claude.tool",
            "Reading file",
            priority::MESSAGE,
            Some("\u{1F4C4}".into()),
            None,
            None,
        );
        let status = store.workspace_status(1).unwrap();
        let entry = status.entry("claude.tool").unwrap();
        assert_eq!(entry.text, "Reading file");
        assert_eq!(entry.priority, priority::MESSAGE);
        assert_eq!(entry.icon.as_deref(), Some("\u{1F4C4}"));

        // Upsert replaces in place.
        store.upsert_entry(
            1,
            "claude.tool",
            "Editing file",
            priority::MESSAGE,
            None,
            None,
            None,
        );
        assert_eq!(
            store
                .workspace_status(1)
                .unwrap()
                .entry("claude.tool")
                .unwrap()
                .text,
            "Editing file"
        );

        // Remove expires the key and leaves others alone.
        store.upsert_entry(
            1,
            "git.branch",
            "main",
            priority::USER_GENERIC,
            None,
            None,
            None,
        );
        assert!(store.remove_entry(1, "claude.tool"));
        let status = store.workspace_status(1).unwrap();
        assert!(status.entry("claude.tool").is_none());
        assert!(status.entry("git.branch").is_some());

        // Double-remove returns false.
        assert!(!store.remove_entry(1, "claude.tool"));
    }

    #[test]
    fn upsert_entry_creates_status_if_missing() {
        let mut store = NotificationStore::new();
        assert!(store.workspace_status(1).is_none());
        store.upsert_entry(
            1,
            "user.generic",
            "hello",
            priority::USER_GENERIC,
            None,
            None,
            None,
        );
        let status = store.workspace_status(1).unwrap();
        assert_eq!(status.state, AgentState::Idle);
        assert_eq!(status.entry("user.generic").unwrap().text, "hello");
    }

    #[test]
    fn entries_by_priority_sorts_descending() {
        let mut store = NotificationStore::new();
        store.upsert_entry(1, "a.low", "low", 10, None, None, None);
        store.upsert_entry(1, "b.high", "high", 100, None, None, None);
        store.upsert_entry(1, "c.mid", "mid", 50, None, None, None);
        // Ties break by key ascending: insert two at priority 50.
        store.upsert_entry(1, "a.tie", "tie", 50, None, None, None);
        let status = store.workspace_status(1).unwrap();
        let ordered: Vec<&str> = status
            .entries_by_priority()
            .iter()
            .map(|(k, _)| *k)
            .collect();
        assert_eq!(ordered, vec!["b.high", "a.tie", "c.mid", "a.low"]);
    }

    #[test]
    fn upsert_entry_rejects_reserved_agent_prefix() {
        let mut store = NotificationStore::new();
        // External publishers must not be able to write the legacy sidebar
        // slots — set_status is the only legitimate writer for "agent.*".
        store.upsert_entry(
            1,
            KEY_AGENT_MESSAGE,
            "should not appear",
            priority::MESSAGE,
            None,
            None,
            None,
        );
        // No workspace status record is created because the write was
        // rejected before reaching the map.
        assert!(store.workspace_status(1).is_none());

        // With a pre-existing status from set_status, the agent.* entry
        // written by set_status survives an external upsert_entry attempt.
        store.set_status(
            2,
            AgentState::Active,
            None,
            None,
            Some("real message".into()),
        );
        store.upsert_entry(
            2,
            KEY_AGENT_MESSAGE,
            "should be rejected",
            priority::MESSAGE,
            None,
            None,
            None,
        );
        assert_eq!(
            store.workspace_status(2).unwrap().message(),
            Some("real message")
        );
    }

    #[test]
    fn upsert_entry_with_ttl_filters_and_prunes() {
        let mut store = NotificationStore::new();
        store.upsert_entry(
            1,
            "claude.tool",
            "Running tool",
            priority::MESSAGE,
            None,
            None,
            Some(Duration::from_millis(50)),
        );
        store.upsert_entry(
            1,
            "git.branch",
            "main",
            priority::USER_GENERIC,
            None,
            None,
            None,
        );

        let status = store.workspace_status(1).unwrap();
        let entry = status.entry("claude.tool").unwrap();
        let entry_expires_at = entry
            .expires_at
            .expect("TTL should be translated to expires_at");

        // Before the deadline both entries are visible.
        let before = entry_expires_at - Duration::from_millis(10);
        let ordered: Vec<&str> = status
            .entries_by_priority_at(before)
            .iter()
            .map(|(k, _)| *k)
            .collect();
        assert_eq!(ordered, vec!["claude.tool", "git.branch"]);

        // After the deadline the TTL-bound entry is filtered from the render
        // list, but the sticky one stays. The raw map is untouched until
        // prune runs.
        let after = entry_expires_at + Duration::from_millis(10);
        let ordered_after: Vec<&str> = status
            .entries_by_priority_at(after)
            .iter()
            .map(|(k, _)| *k)
            .collect();
        assert_eq!(ordered_after, vec!["git.branch"]);
        assert!(status.entries.contains_key("claude.tool"));

        // Prune reclaims the expired slot. Sticky entry is left alone.
        // Uses the `_at` variant so the test is deterministic rather than
        // racing a real sleep.
        let removed = store.prune_expired_entries_at(1, after);
        assert_eq!(removed, 1);
        let status = store.workspace_status(1).unwrap();
        assert!(!status.entries.contains_key("claude.tool"));
        assert!(status.entries.contains_key("git.branch"));

        // Prune on an unknown workspace is a no-op.
        assert_eq!(store.prune_expired_entries_at(999, after), 0);
    }

    #[test]
    fn legacy_slots_are_sticky_not_ttl() {
        // set_status never attaches a TTL — the legacy sidebar slots must
        // live until overwritten.
        let mut store = NotificationStore::new();
        store.set_status(
            1,
            AgentState::Active,
            Some("Running".into()),
            None,
            Some("latest message".into()),
        );
        let status = store.workspace_status(1).unwrap();
        assert!(status
            .entry(KEY_AGENT_LABEL)
            .expect("label entry")
            .expires_at
            .is_none());
        assert!(status
            .entry(KEY_AGENT_MESSAGE)
            .expect("message entry")
            .expires_at
            .is_none());
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
        store.push(
            1,
            10,
            100,
            "A".into(),
            String::new(),
            "a".into(),
            NotificationSource::Bell,
        );
        store.push(
            2,
            20,
            200,
            "B".into(),
            String::new(),
            "b".into(),
            NotificationSource::Cli,
        );
        store.clear_all();
        assert_eq!(store.all_notifications().len(), 0);
        assert_eq!(store.pane_unread(10), 0);
        assert_eq!(store.pane_unread(20), 0);
    }

    #[test]
    fn mark_workspace_read_only_affects_given_panes() {
        let mut store = NotificationStore::new();
        // Workspace 1 panes: 10, 11
        store.push(
            1,
            10,
            100,
            "A".into(),
            String::new(),
            "a".into(),
            NotificationSource::Bell,
        );
        store.push(
            1,
            11,
            101,
            "B".into(),
            String::new(),
            "b".into(),
            NotificationSource::Bell,
        );
        // Workspace 2 pane: 20
        store.push(
            2,
            20,
            200,
            "C".into(),
            String::new(),
            "c".into(),
            NotificationSource::Bell,
        );

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
            String::new(),
            "a".into(),
            NotificationSource::Bell,
        );
        store.push(
            2,
            20,
            200,
            "Other".into(),
            String::new(),
            "b".into(),
            NotificationSource::Bell,
        );
        store.push(
            1,
            10,
            101,
            "Latest".into(),
            String::new(),
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
    fn total_unread_across_panes() {
        let mut store = NotificationStore::new();
        store.push(
            1,
            10,
            100,
            "A".into(),
            String::new(),
            "a".into(),
            NotificationSource::Bell,
        );
        store.push(
            1,
            10,
            101,
            "B".into(),
            String::new(),
            "b".into(),
            NotificationSource::Bell,
        );
        store.push(
            2,
            20,
            200,
            "C".into(),
            String::new(),
            "c".into(),
            NotificationSource::Cli,
        );
        assert_eq!(store.total_unread(), 3);

        store.mark_pane_read(10);
        assert_eq!(store.total_unread(), 1);

        store.remove_pane(20);
        assert_eq!(store.total_unread(), 0);
    }

    #[test]
    fn progress_lifecycle() {
        let mut store = NotificationStore::new();
        // set_status creates entry with no progress
        store.set_status(1, AgentState::Active, Some("Building".into()), None, None);
        assert!(store.workspace_status(1).unwrap().progress.is_none());
        assert!(store.workspace_status(1).unwrap().progress_label.is_none());

        // set_progress adds progress
        store.set_progress(1, Some(0.5), None);
        assert_eq!(store.workspace_status(1).unwrap().progress, Some(0.5));

        // set_progress with a label stores both
        store.set_progress(1, Some(0.75), Some("compiling 75/100".into()));
        assert_eq!(store.workspace_status(1).unwrap().progress, Some(0.75));
        assert_eq!(
            store.workspace_status(1).unwrap().progress_label.as_deref(),
            Some("compiling 75/100")
        );

        // Clearing progress also drops the label — an orphan label with
        // no bar has nowhere to render.
        store.set_progress(1, None, Some("stale".into()));
        assert!(store.workspace_status(1).unwrap().progress.is_none());
        assert!(store.workspace_status(1).unwrap().progress_label.is_none());

        // Restore a bar+label, then verify set_status clears both.
        store.set_progress(1, Some(0.25), Some("step 1".into()));
        store.set_status(1, AgentState::Idle, None, None, None);
        assert!(store.workspace_status(1).unwrap().progress.is_none());
        assert!(store.workspace_status(1).unwrap().progress_label.is_none());
    }

    #[test]
    fn set_progress_on_missing_workspace_is_noop() {
        // `set_progress` is a decoration on top of an existing row; it
        // shouldn't lazily allocate one just to stash a value.
        let mut store = NotificationStore::new();
        store.set_progress(99, Some(0.5), Some("ignored".into()));
        assert!(store.workspace_status(99).is_none());
    }

    #[test]
    fn push_supersedes_prior_same_surface() {
        let mut store = NotificationStore::new();
        let first = store.push(
            1,
            10,
            100,
            "First".into(),
            String::new(),
            "old".into(),
            NotificationSource::Bell,
        );
        store.push(
            1,
            10,
            100,
            "Second".into(),
            String::new(),
            "new".into(),
            NotificationSource::Bell,
        );
        // Only the newest notification for this surface should remain.
        assert_eq!(store.all_notifications().len(), 1);
        assert_eq!(store.all_notifications()[0].title, "Second");
        // Unread count tracks surviving notification, not accumulation.
        assert_eq!(store.pane_unread(10), 1);
        // The superseded id should not match the surviving one.
        assert_ne!(store.all_notifications()[0].id, first);
    }

    #[test]
    fn push_preserves_other_surfaces() {
        let mut store = NotificationStore::new();
        store.push(
            1,
            10,
            100,
            "A".into(),
            String::new(),
            "a".into(),
            NotificationSource::Bell,
        );
        store.push(
            1,
            10,
            101, // different surface
            "B".into(),
            String::new(),
            "b".into(),
            NotificationSource::Bell,
        );
        store.push(
            2, // different workspace
            20,
            100,
            "C".into(),
            String::new(),
            "c".into(),
            NotificationSource::Bell,
        );
        assert_eq!(store.all_notifications().len(), 3);
        assert_eq!(store.pane_unread(10), 2);
        assert_eq!(store.pane_unread(20), 1);
    }

    #[test]
    fn push_preserves_read_history_for_surface() {
        let mut store = NotificationStore::new();
        store.push(
            1,
            10,
            100,
            "Old".into(),
            String::new(),
            "old".into(),
            NotificationSource::Bell,
        );
        store.mark_pane_read(10);
        assert!(store.all_notifications()[0].read);

        // A new notification for the same surface must NOT supersede a read
        // entry — read notifications are preserved as history (issue #38).
        store.push(
            1,
            10,
            100,
            "New".into(),
            String::new(),
            "new".into(),
            NotificationSource::Bell,
        );
        let all = store.all_notifications();
        assert_eq!(all.len(), 2);
        // Order is insertion order: the preserved read entry is first, the
        // new unread entry is appended after it.
        assert_eq!(all[0].title, "Old");
        assert!(all[0].read);
        assert_eq!(all[1].title, "New");
        assert!(!all[1].read);
        assert_eq!(store.pane_unread(10), 1);
    }

    #[test]
    fn push_read_also_supersedes() {
        let mut store = NotificationStore::new();
        // Start with an unread notification for the surface.
        store.push(
            1,
            10,
            100,
            "Unread".into(),
            String::new(),
            "u".into(),
            NotificationSource::Bell,
        );
        assert_eq!(store.pane_unread(10), 1);

        // push_read for the same surface supersedes prior unread entries
        // and correctly decrements the unread count.
        store.push_read(
            1,
            10,
            100,
            "Focused".into(),
            String::new(),
            "f".into(),
            NotificationSource::Toast,
        );
        assert_eq!(store.all_notifications().len(), 1);
        assert_eq!(store.all_notifications()[0].title, "Focused");
        assert!(store.all_notifications()[0].read);
        assert_eq!(store.pane_unread(10), 0);
    }

    #[test]
    fn push_restored_preserves_history_without_flash() {
        let mut store = NotificationStore::new();
        // Two restored entries for the same (workspace, surface) must both
        // survive — push_restored is the session-restore path and must not
        // collapse history the way push()/push_read() do.
        store.push_restored(
            1,
            10,
            100,
            "First".into(),
            String::new(),
            "a".into(),
            NotificationSource::Bell,
        );
        store.push_restored(
            1,
            10,
            100,
            "Second".into(),
            String::new(),
            "b".into(),
            NotificationSource::Bell,
        );
        let all = store.all_notifications();
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|n| n.read));
        // Restored entries are read, so unread count stays at zero and no
        // pane_state (and thus no flash) is created by the restore path.
        assert_eq!(store.pane_unread(10), 0);
        assert!(store.pane_state(10).is_none());
    }

    // --- G3: debounced displayed snapshot ----------------------------------

    const DEBOUNCE: Duration = Duration::from_millis(40);

    fn new_ws(store: &mut NotificationStore, ws: u64) {
        // Touch set_status to materialize the workspace entry.
        store.set_status(ws, AgentState::Idle, None, None, None);
    }

    #[test]
    fn fresh_upsert_is_held_until_debounce_elapses() {
        let mut store = NotificationStore::new();
        new_ws(&mut store, 1);
        let t0 = Instant::now();
        store.upsert_entry_at(1, "claude.tool", "Running cargo", 60, None, None, None, t0);
        // Commit right at the write instant → still under debounce, hold.
        store.commit_displayed_at(t0, DEBOUNCE);
        let status = store.workspace_status(1).unwrap();
        assert!(status.displayed.is_empty(), "must wait for debounce");

        // Commit a tick after the window → now visible.
        store.commit_displayed_at(t0 + DEBOUNCE, DEBOUNCE);
        let status = store.workspace_status(1).unwrap();
        assert_eq!(
            status.displayed.get("claude.tool").map(|e| e.text.as_str()),
            Some("Running cargo")
        );
    }

    #[test]
    fn transient_write_removed_within_debounce_is_never_shown() {
        // Regression for the flicker bug class this whole feature targets:
        // a rapid upsert → remove burst must NOT flash the value on screen.
        let mut store = NotificationStore::new();
        new_ws(&mut store, 1);
        let t0 = Instant::now();
        store.upsert_entry_at(1, "claude.tool", "Running test", 60, None, None, None, t0);
        store.remove_entry_at(1, "claude.tool", t0 + Duration::from_millis(10));
        store.commit_displayed_at(t0 + Duration::from_millis(50), DEBOUNCE);
        let status = store.workspace_status(1).unwrap();
        assert!(
            status.displayed.is_empty(),
            "upsert-then-remove inside the debounce window must not render"
        );
    }

    #[test]
    fn removal_after_display_holds_for_debounce_then_drops() {
        let mut store = NotificationStore::new();
        new_ws(&mut store, 1);
        let t0 = Instant::now();
        store.upsert_entry_at(1, "claude.tool", "Running test", 60, None, None, None, t0);
        // Promote the value to displayed.
        store.commit_displayed_at(t0 + DEBOUNCE, DEBOUNCE);
        assert_eq!(
            store.workspace_status(1).unwrap().displayed.len(),
            1,
            "precondition: value must be visible"
        );

        // Remove the entry; commit immediately — displayed must still hold.
        let t_remove = t0 + DEBOUNCE + Duration::from_millis(5);
        store.remove_entry_at(1, "claude.tool", t_remove);
        store.commit_displayed_at(t_remove, DEBOUNCE);
        assert_eq!(
            store.workspace_status(1).unwrap().displayed.len(),
            1,
            "removal must not vanish the displayed value inside the window"
        );

        // After the full debounce window, displayed drops the entry.
        store.commit_displayed_at(t_remove + DEBOUNCE, DEBOUNCE);
        assert!(store.workspace_status(1).unwrap().displayed.is_empty());
        assert!(store
            .workspace_status(1)
            .unwrap()
            .pending_removals
            .is_empty());
    }

    #[test]
    fn reinsert_within_window_cancels_tombstone() {
        // upsert → show → remove → reinsert within debounce: the final
        // state must persist; we must not drop the re-inserted value when
        // the old tombstone would otherwise have expired.
        let mut store = NotificationStore::new();
        new_ws(&mut store, 1);
        let t0 = Instant::now();
        store.upsert_entry_at(1, "claude.tool", "A", 60, None, None, None, t0);
        store.commit_displayed_at(t0 + DEBOUNCE, DEBOUNCE);

        let t1 = t0 + DEBOUNCE + Duration::from_millis(5);
        store.remove_entry_at(1, "claude.tool", t1);
        // Reinsert a few ms later, still inside the tombstone window.
        let t2 = t1 + Duration::from_millis(5);
        store.upsert_entry_at(1, "claude.tool", "B", 60, None, None, None, t2);

        // Step far past both the original tombstone expiry and the new
        // entry's debounce window. Displayed should now be "B".
        store.commit_displayed_at(t2 + DEBOUNCE + Duration::from_millis(50), DEBOUNCE);
        let status = store.workspace_status(1).unwrap();
        assert_eq!(
            status.displayed.get("claude.tool").map(|e| e.text.as_str()),
            Some("B"),
            "re-inserted value must win over the stale tombstone"
        );
        assert!(status.pending_removals.is_empty());
    }

    #[test]
    fn next_commit_at_reports_earliest_deadline() {
        let mut store = NotificationStore::new();
        new_ws(&mut store, 1);
        let t0 = Instant::now();
        store.upsert_entry_at(1, "claude.tool", "first", 60, None, None, None, t0);
        // Next commit = t0 + DEBOUNCE.
        let wake = store.next_commit_at(DEBOUNCE).unwrap();
        assert_eq!(
            wake - t0,
            DEBOUNCE,
            "wake ({:?} after t0) should be exactly DEBOUNCE",
            wake - t0
        );

        // After commit, same entry is now displayed. next_commit_at should
        // stop reporting a deadline for it.
        store.commit_displayed_at(wake, DEBOUNCE);
        assert!(store.next_commit_at(DEBOUNCE).is_none());
    }

    #[test]
    fn legacy_field_clear_tombstones_the_message_slot() {
        // A set_status with Some("") used to be the flicker trigger —
        // make sure the G3 tombstone path holds the displayed message for
        // the debounce window on that path too.
        let mut store = NotificationStore::new();
        let t0 = Instant::now();
        store.set_status_at(
            1,
            AgentState::Active,
            None,
            None,
            Some("Running cargo".into()),
            t0,
        );
        store.commit_displayed_at(t0 + DEBOUNCE, DEBOUNCE);
        assert_eq!(
            store.workspace_status(1).unwrap().displayed_message(),
            Some("Running cargo")
        );

        // Clear via set_status(Some("")) and commit right away — displayed
        // must still hold the old message for the debounce window.
        let t_clear = t0 + DEBOUNCE + Duration::from_millis(5);
        store.set_status_at(
            1,
            AgentState::Active,
            None,
            None,
            Some(String::new()),
            t_clear,
        );
        store.commit_displayed_at(t_clear, DEBOUNCE);
        assert_eq!(
            store.workspace_status(1).unwrap().displayed_message(),
            Some("Running cargo"),
            "displayed must not blank mid-debounce"
        );
    }
}
