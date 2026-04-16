//! Shared emission plumbing for per-agent hook handlers.
//!
//! Each hook handler (Claude, Gemini, Codex) maps an incoming event into a
//! list of `HookAction`s that get dispatched to the IPC server. This lets the
//! pure `*_actions_for(event, data, ws_id)` helpers stay synchronous and
//! trivially unit-testable, while `handle_*_hook` owns the I/O.
//!
//! # Why actions (G23)
//!
//! Pre-parity, each hook emitted a single `status.set` call. That single-slot
//! model is what produced the original flicker: `PostToolUse` set
//! `message: ""` to clear the per-tool label, blanking the row between
//! consecutive tool calls within one turn.
//!
//! The parity plan ([#260](https://github.com/daveowenatl/amux/issues/260)
//! gap G23) converts each hook to publish under its own namespaced key
//! (`claude.tool`, `gemini.tool`, `codex.tool`, `claude.notification`,
//! `claude.subagent`). Tool-end hooks call `remove_entry` on their key
//! rather than blanking `agent.message`, so the other publishers' entries
//! stay visible.
//!
//! For back-compat with the legacy single-message sidebar (until G20 lands
//! and the renderer iterates `entries_by_priority`), hooks *also* write the
//! same text into `agent.message` via `status.set`, and — crucially — tool-end
//! events no longer clear it. The last tool's message sticks until the next
//! tool call overwrites it, which is what actually fixes the flicker on the
//! current sidebar.

use amux_ipc::IpcClient;
use serde_json::{json, Value};

/// What a hook handler wants done on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HookAction {
    /// Mirrors the legacy `status.set` call: updates agent state, label,
    /// task, and/or the legacy `agent.message` slot. Fields omitted from
    /// the params preserve their prior value (see `apply_legacy_field`).
    SetStatus(Value),
    /// Publishes or replaces a keyed status entry for this workspace.
    /// Dispatched as `status.upsert_entry`.
    UpsertEntry {
        key: String,
        text: String,
        priority: i32,
    },
    /// Expires a keyed status entry. Dispatched as `status.remove_entry`.
    RemoveEntry { key: String },
}

impl HookAction {
    /// Build an `UpsertEntry` action. Shortcut so hook handlers can chain
    /// one-liners without the `String::from` / field boilerplate.
    pub(crate) fn upsert(key: &str, text: impl Into<String>, priority: i32) -> Self {
        HookAction::UpsertEntry {
            key: key.to_string(),
            text: text.into(),
            priority,
        }
    }

    /// Build a `RemoveEntry` action.
    pub(crate) fn remove(key: &str) -> Self {
        HookAction::RemoveEntry {
            key: key.to_string(),
        }
    }
}

/// Dispatch a list of actions over the IPC client. `UpsertEntry` /
/// `RemoveEntry` calls are best-effort: if the server is running an older
/// amux build that doesn't know about `status.upsert_entry`, we still
/// want `status.set` calls to go through. Errors on those two methods are
/// therefore swallowed.
pub(crate) async fn dispatch_actions(
    client: &mut IpcClient,
    ws_id: &str,
    actions: Vec<HookAction>,
) -> anyhow::Result<()> {
    for action in actions {
        match action {
            HookAction::SetStatus(params) => {
                client.call("status.set", params).await?;
            }
            HookAction::UpsertEntry {
                key,
                text,
                priority,
            } => {
                let _ = client
                    .call(
                        "status.upsert_entry",
                        json!({
                            "workspace_id": ws_id,
                            "key": key,
                            "text": text,
                            "priority": priority,
                        }),
                    )
                    .await;
            }
            HookAction::RemoveEntry { key } => {
                let _ = client
                    .call(
                        "status.remove_entry",
                        json!({
                            "workspace_id": ws_id,
                            "key": key,
                        }),
                    )
                    .await;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Priority constants used across hooks
// ---------------------------------------------------------------------------
//
// These live here rather than in `amux_notify::priority` because they're
// hook-emitter conventions, not core store semantics — and amux-cli doesn't
// otherwise depend on amux-notify. If the set ever grows large we can pull
// amux-notify in as a dep and re-export. Matches cmux's ordering:
// notification > subagent > tool, so a "Needs input" entry wins over an
// in-progress tool label in the sorted render list once G20 lands.
//
// Values align with `amux_notify::priority::MESSAGE` (60) for the tool
// slot — hooks render at parity with the legacy `agent.message` bucket.

/// Priority for `<agent>.notification` keys. Sits between `MESSAGE` (60) and
/// `TASK` (80) — above generic tool messages but below the task/prompt title
/// so "Needs input" overlays in-progress work without hiding it.
pub(crate) const NOTIFICATION_PRIORITY: i32 = 70;

/// Priority for `<agent>.subagent` keys. Slightly above a plain tool message
/// so a subagent in flight wins over whatever tool the parent last ran.
pub(crate) const SUBAGENT_PRIORITY: i32 = 65;

/// Priority for `<agent>.tool` keys. Matches
/// `amux_notify::priority::MESSAGE` (60) so tool-label entries render at
/// parity with the legacy `agent.message` slot.
pub(crate) const TOOL_PRIORITY: i32 = 60;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_builds_action() {
        let a = HookAction::upsert("claude.tool", "Running", TOOL_PRIORITY);
        let HookAction::UpsertEntry {
            key,
            text,
            priority,
        } = a
        else {
            panic!("expected UpsertEntry");
        };
        assert_eq!(key, "claude.tool");
        assert_eq!(text, "Running");
        // TOOL_PRIORITY mirrors amux_notify::priority::MESSAGE (60) — if
        // core changes its MESSAGE priority this assertion will catch the
        // hook emitter drifting out of sync with the renderer.
        assert_eq!(priority, 60);
    }

    #[test]
    fn remove_builds_action() {
        let a = HookAction::remove("claude.tool");
        let HookAction::RemoveEntry { key } = a else {
            panic!("expected RemoveEntry");
        };
        assert_eq!(key, "claude.tool");
    }

    #[test]
    fn priorities_ordered_notification_subagent_tool() {
        assert!(NOTIFICATION_PRIORITY > SUBAGENT_PRIORITY);
        assert!(SUBAGENT_PRIORITY > TOOL_PRIORITY);
    }
}
