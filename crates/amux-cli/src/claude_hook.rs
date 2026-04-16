//! Claude Code hook event handling.
//!
//! Reads JSON from stdin and translates Claude Code hook events
//! (PreToolUse, PostToolUse, Stop, etc.) into amux IPC calls that
//! update workspace status, send notifications, and manage agent state.
//!
//! Per parity plan gap G23, each event publishes to its own namespaced
//! key (`claude.tool`, `claude.notification`, `claude.subagent`) via
//! `status.upsert_entry` / `status.remove_entry`, so expiring one
//! publisher's entry doesn't blank another publisher's content. The
//! legacy single-message slot is still dual-written via `status.set` for
//! back-compat with the current sidebar (which still reads
//! `agent.message` directly); tool-end events notably *do not* clear
//! that slot — they just remove their keyed entry, leaving the last
//! tool's message to persist until the next PreToolUse overwrites it.
//! This is the piece that actually closes the original flicker.

use crate::hook_action::{
    dispatch_actions, HookAction, NOTIFICATION_PRIORITY, SUBAGENT_PRIORITY, TOOL_PRIORITY,
};
use amux_ipc::IpcClient;
use serde_json::{json, Value};
use std::io::Read as _;

/// Publisher-owned keys for Claude hook emissions.
pub(crate) const KEY_TOOL: &str = "claude.tool";
pub(crate) const KEY_SUBAGENT: &str = "claude.subagent";
pub(crate) const KEY_NOTIFICATION: &str = "claude.notification";

/// Pure helper: map a Claude Code hook event + payload to a list of
/// [`HookAction`]s. Returns an empty Vec when the event is observed but
/// produces no status change.
pub(crate) fn hook_actions(event: &str, data: &Value, ws_id: &str) -> Vec<HookAction> {
    match event {
        "SessionStart" => vec![
            HookAction::SetStatus(json!({
                "workspace_id": ws_id,
                "state": "idle",
                "label": "Idle",
                "task": "",
                "message": "",
            })),
            // Fresh session: drop any lingering claude.* entries from a
            // prior session that crashed before `Stop`.
            HookAction::remove(KEY_TOOL),
            HookAction::remove(KEY_SUBAGENT),
            HookAction::remove(KEY_NOTIFICATION),
        ],
        "UserPromptSubmit" => {
            let prompt = data.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
            let task = truncate(prompt, 80);
            let mut params = json!({
                "workspace_id": ws_id,
                "state": "active",
                "label": "Running",
                // New user prompt starts a fresh turn: clear the legacy
                // per-tool message from the previous turn so the sidebar
                // doesn't show stale "Running cargo test" after the user
                // has already replied.
                "message": "",
            });
            if !task.is_empty() {
                params["task"] = json!(task);
            }
            vec![
                HookAction::SetStatus(params),
                // Prompt answered → any pending "needs input" notification is
                // no longer relevant.
                HookAction::remove(KEY_NOTIFICATION),
                // Prior turn's tool/subagent entries shouldn't carry into
                // the new turn.
                HookAction::remove(KEY_TOOL),
                HookAction::remove(KEY_SUBAGENT),
            ]
        }
        "PreToolUse" => {
            let tool_name = data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
            let description = describe_tool_use(tool_name, data.get("tool_input"));
            // Dual-write: (a) keyed `claude.tool` entry for G20-era
            // entries_by_priority rendering; (b) legacy `agent.message`
            // slot for today's sidebar.
            vec![
                HookAction::SetStatus(json!({
                    "workspace_id": ws_id,
                    "state": "active",
                    "label": "Running",
                    "message": description,
                })),
                HookAction::upsert(KEY_TOOL, description, TOOL_PRIORITY),
            ]
        }
        "PostToolUse" => {
            // Original flicker fix: no longer clear `agent.message`. We
            // only remove the keyed entry; the legacy message persists
            // until the next PreToolUse overwrites it, so consecutive
            // tool calls no longer produce a blank frame between them.
            //
            // We also skip `status.set` entirely — state/label are still
            // active from the prior PreToolUse, and not sending the call
            // avoids the store bumping `updated_at` with identical data.
            vec![HookAction::remove(KEY_TOOL)]
        }
        "Notification" => {
            // Claude needs attention. Claude's Notification payload includes
            // a `message` field with context-specific text (permission prompt,
            // idle warning, etc.) — surface it rather than the generic label.
            let message = data.get("message").and_then(|v| v.as_str()).unwrap_or("");
            vec![
                HookAction::SetStatus(json!({
                    "workspace_id": ws_id,
                    "state": "waiting",
                    "label": "Needs input",
                    "message": message,
                })),
                HookAction::upsert(KEY_NOTIFICATION, message, NOTIFICATION_PRIORITY),
            ]
        }
        "Stop" => vec![
            HookAction::SetStatus(json!({
                "workspace_id": ws_id,
                "state": "idle",
                "label": "Idle",
                "task": "",
                "message": "",
            })),
            // Turn is over; any per-tool / per-subagent / notification
            // entries should not leak into the idle row.
            HookAction::remove(KEY_TOOL),
            HookAction::remove(KEY_SUBAGENT),
            HookAction::remove(KEY_NOTIFICATION),
        ],
        "SubagentStart" => {
            let agent_name = data
                .get("agent_name")
                .and_then(|v| v.as_str())
                .unwrap_or("subagent");
            let msg = format!("Running {agent_name}");
            vec![
                HookAction::SetStatus(json!({
                    "workspace_id": ws_id,
                    "state": "active",
                    "label": "Running",
                    "message": msg,
                })),
                HookAction::upsert(KEY_SUBAGENT, msg, SUBAGENT_PRIORITY),
            ]
        }
        "SubagentStop" => {
            // Mirror of PostToolUse: don't blank the legacy message; parent
            // hook events will overwrite it on the next tool call or Stop.
            vec![HookAction::remove(KEY_SUBAGENT)]
        }
        "SessionEnd" => vec![
            HookAction::SetStatus(json!({
                "workspace_id": ws_id,
                "state": "idle",
                "label": "Idle",
                "task": "",
                "message": "",
            })),
            HookAction::remove(KEY_TOOL),
            HookAction::remove(KEY_SUBAGENT),
            HookAction::remove(KEY_NOTIFICATION),
        ],
        _ => Vec::new(),
    }
}

pub async fn handle_claude_hook(client: &mut IpcClient, event: &str) -> anyhow::Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let data: Value = serde_json::from_str(&input).unwrap_or_default();
    let ws_id = std::env::var("AMUX_WORKSPACE_ID").unwrap_or_else(|_| "0".to_string());

    let actions = hook_actions(event, &data, &ws_id);
    dispatch_actions(client, &ws_id, actions).await?;

    // "Notification" means Claude needs input. In addition to updating
    // the status pill (above), deliver a stored notification so the
    // pane ring, sidebar badge, and auto_reorder_workspaces all fire.
    if event == "Notification" {
        let surface_id = std::env::var("AMUX_SURFACE_ID").unwrap_or_else(|_| "0".to_string());
        let pane_id = surface_id.clone();
        let message = data
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("Needs input");
        let _ = client
            .call(
                "notify.send",
                json!({
                    "workspace_id": ws_id,
                    "pane_id": pane_id,
                    "surface_id": surface_id,
                    "title": "Claude Code",
                    "body": message,
                }),
            )
            .await;
    }

    Ok(())
}

/// Generate a human-readable description of a tool use, matching cmux's
/// `describeToolUse()`.
fn describe_tool_use(tool_name: &str, tool_input: Option<&Value>) -> String {
    let null = Value::Null;
    let input = tool_input.unwrap_or(&null);

    match tool_name {
        "Read" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format_with_target("Reading", filename_of(path), "file")
        }
        "Edit" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format_with_target("Editing", filename_of(path), "file")
        }
        "Write" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format_with_target("Writing", filename_of(path), "file")
        }
        "Bash" => {
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            format_with_target("Running", &truncate(cmd, 60), "command")
        }
        "Glob" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("*");
            format_with_target("Searching", pattern, "files")
        }
        "Grep" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            format_with_target("Grep", pattern, "files")
        }
        "WebFetch" => "Fetching URL".to_string(),
        "WebSearch" => {
            let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
            format_with_target("Search:", query, "web")
        }
        "Agent" => {
            let desc = input
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("subagent");
            desc.to_string()
        }
        _ => tool_name.to_string(),
    }
}

/// Produce `"<verb> <target>"`, falling back to `"<verb> <fallback>"`
/// when the target is empty so we never render trailing whitespace.
fn format_with_target(verb: &str, target: &str, fallback: &str) -> String {
    if target.is_empty() {
        format!("{verb} {fallback}")
    } else {
        format!("{verb} {target}")
    }
}

fn filename_of(path: &str) -> &str {
    std::path::Path::new(path)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(path)
}

/// Truncate a string to at most `max` characters, ending with "..." when
/// shortened. Counts characters, not bytes, to stay safe with UTF-8.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(3);
    let mut out: String = s.chars().take(keep).collect();
    out.push_str("...");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Pull the first `SetStatus` action's params out, panicking if there
    /// isn't one. Keeps the legacy assertions readable in tests that focus
    /// on the status.set payload.
    fn set_status_params(actions: &[HookAction]) -> &Value {
        for a in actions {
            if let HookAction::SetStatus(v) = a {
                return v;
            }
        }
        panic!("no SetStatus action in {actions:?}");
    }

    /// Collect all upsert keys+text from an action list.
    fn upserts(actions: &[HookAction]) -> Vec<(&str, &str, i32)> {
        actions
            .iter()
            .filter_map(|a| match a {
                HookAction::UpsertEntry {
                    key,
                    text,
                    priority,
                } => Some((key.as_str(), text.as_str(), *priority)),
                _ => None,
            })
            .collect()
    }

    /// Collect all remove keys from an action list.
    fn removes(actions: &[HookAction]) -> Vec<&str> {
        actions
            .iter()
            .filter_map(|a| match a {
                HookAction::RemoveEntry { key } => Some(key.as_str()),
                _ => None,
            })
            .collect()
    }

    // ---- SessionStart / SessionEnd ----

    #[test]
    fn session_start_sets_idle_and_clears_claude_keys() {
        let payload = json!({ "hook_event_name": "SessionStart" });
        let actions = hook_actions("SessionStart", &payload, "42");
        let params = set_status_params(&actions);
        assert_eq!(params["workspace_id"], "42");
        assert_eq!(params["state"], "idle");
        assert_eq!(params["label"], "Idle");
        assert_eq!(params["task"], "");
        assert_eq!(params["message"], "");
        // All claude.* keys wiped on fresh session start.
        let mut r = removes(&actions);
        r.sort();
        assert_eq!(
            r,
            vec!["claude.notification", "claude.subagent", "claude.tool"]
        );
    }

    #[test]
    fn session_end_sets_idle_and_clears_keys() {
        let payload = json!({ "hook_event_name": "SessionEnd" });
        let actions = hook_actions("SessionEnd", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "idle");
        assert_eq!(params["task"], "");
        assert_eq!(params["message"], "");
        let mut r = removes(&actions);
        r.sort();
        assert_eq!(
            r,
            vec!["claude.notification", "claude.subagent", "claude.tool"]
        );
    }

    // ---- UserPromptSubmit ----

    #[test]
    fn user_prompt_submit_sets_active_with_prompt_and_clears_prior_keys() {
        let payload = json!({ "prompt": "refactor the auth module" });
        let actions = hook_actions("UserPromptSubmit", &payload, "42");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "active");
        assert_eq!(params["label"], "Running");
        assert_eq!(params["task"], "refactor the auth module");
        // Prompt answered: any prior-turn claude.* entries go away.
        let mut r = removes(&actions);
        r.sort();
        assert_eq!(
            r,
            vec!["claude.notification", "claude.subagent", "claude.tool"]
        );
    }

    #[test]
    fn user_prompt_submit_without_prompt_omits_task() {
        let payload = json!({});
        let actions = hook_actions("UserPromptSubmit", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "active");
        assert!(params.get("task").is_none() || params["task"] == "");
    }

    #[test]
    fn user_prompt_submit_truncates_long_prompts() {
        let long = "a".repeat(200);
        let payload = json!({ "prompt": long });
        let actions = hook_actions("UserPromptSubmit", &payload, "1");
        let params = set_status_params(&actions);
        let task = params["task"].as_str().unwrap();
        assert_eq!(task.chars().count(), 80);
        assert!(task.ends_with("..."));
    }

    // ---- PreToolUse ----

    #[test]
    fn pre_tool_use_bash_writes_legacy_message_and_keyed_entry() {
        let payload = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test" }
        });
        let actions = hook_actions("PreToolUse", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "active");
        assert_eq!(
            params["message"], "Running cargo test",
            "legacy agent.message dual-write must still carry the tool label"
        );
        assert_eq!(
            upserts(&actions),
            vec![("claude.tool", "Running cargo test", TOOL_PRIORITY)]
        );
    }

    #[test]
    fn pre_tool_use_read_shows_filename() {
        let payload = json!({
            "tool_name": "Read",
            "tool_input": { "file_path": "/Users/me/project/src/main.rs" }
        });
        let actions = hook_actions("PreToolUse", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["message"], "Reading main.rs");
        assert_eq!(
            upserts(&actions),
            vec![("claude.tool", "Reading main.rs", TOOL_PRIORITY)]
        );
    }

    #[test]
    fn pre_tool_use_missing_path_falls_back_to_generic_label() {
        let payload = json!({ "tool_name": "Read", "tool_input": {} });
        let actions = hook_actions("PreToolUse", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["message"], "Reading file");

        let payload = json!({ "tool_name": "Bash", "tool_input": {} });
        let actions = hook_actions("PreToolUse", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["message"], "Running command");
    }

    // ---- PostToolUse ----

    /// Regression for the original flicker bug: PostToolUse must NOT emit
    /// a `status.set` that clears the legacy message. It only expires the
    /// keyed `claude.tool` entry, leaving agent.message intact until the
    /// next PreToolUse overwrites it. Between two consecutive tool calls
    /// in the same turn this is what eliminates the blank frame.
    #[test]
    fn post_tool_use_only_removes_keyed_entry_does_not_blank_legacy_message() {
        let payload = json!({ "tool_name": "Bash", "tool_output": "..." });
        let actions = hook_actions("PostToolUse", &payload, "1");
        assert!(
            !actions.iter().any(|a| matches!(a, HookAction::SetStatus(_))),
            "PostToolUse must not emit status.set (that's what blanked agent.message and caused the flicker)"
        );
        assert_eq!(removes(&actions), vec!["claude.tool"]);
    }

    // ---- Notification ----

    #[test]
    fn notification_sets_waiting_and_surfaces_message_and_keyed_entry() {
        let payload = json!({
            "message": "Claude needs your permission to run a command",
            "title": "Permission request"
        });
        let actions = hook_actions("Notification", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "waiting");
        assert_eq!(params["label"], "Needs input");
        assert_eq!(
            params["message"],
            "Claude needs your permission to run a command"
        );
        assert_eq!(
            upserts(&actions),
            vec![(
                "claude.notification",
                "Claude needs your permission to run a command",
                NOTIFICATION_PRIORITY
            )]
        );
    }

    /// Regression: Notification without a payload `message` must still send
    /// `message: ""` so the IPC `status.set` handler clears any stale
    /// per-tool message from a prior PreToolUse. Passing nothing would
    /// preserve the previous message and leave the UI showing "Running
    /// cargo test" while the state has already flipped to "waiting".
    #[test]
    fn notification_without_message_sends_empty_string_to_clear() {
        let payload = json!({});
        let actions = hook_actions("Notification", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "waiting");
        assert_eq!(params["label"], "Needs input");
        assert_eq!(
            params["message"], "",
            "message must be explicit empty string, not absent, so set_status clears"
        );
    }

    // ---- Stop ----

    #[test]
    fn stop_goes_idle_and_clears_all_keys() {
        let payload = json!({});
        let actions = hook_actions("Stop", &payload, "9");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "idle");
        assert_eq!(params["task"], "");
        assert_eq!(params["message"], "");
        let mut r = removes(&actions);
        r.sort();
        assert_eq!(
            r,
            vec!["claude.notification", "claude.subagent", "claude.tool"]
        );
    }

    // ---- SubagentStart / SubagentStop ----

    #[test]
    fn subagent_start_shows_agent_name_in_legacy_and_keyed_entry() {
        let payload = json!({ "agent_name": "code-reviewer" });
        let actions = hook_actions("SubagentStart", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "active");
        assert_eq!(params["message"], "Running code-reviewer");
        assert_eq!(
            upserts(&actions),
            vec![(
                "claude.subagent",
                "Running code-reviewer",
                SUBAGENT_PRIORITY
            )]
        );
    }

    #[test]
    fn subagent_start_without_name_uses_default() {
        let payload = json!({});
        let actions = hook_actions("SubagentStart", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["message"], "Running subagent");
    }

    /// Mirror of `post_tool_use_only_removes_keyed_entry_does_not_blank_legacy_message`
    /// for subagents. Subagent end must not blank the legacy message either.
    #[test]
    fn subagent_stop_only_removes_keyed_entry() {
        let payload = json!({ "agent_name": "code-reviewer" });
        let actions = hook_actions("SubagentStop", &payload, "1");
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, HookAction::SetStatus(_))),
            "SubagentStop must not emit status.set"
        );
        assert_eq!(removes(&actions), vec!["claude.subagent"]);
    }

    // ---- Unknown events ----

    #[test]
    fn unknown_event_does_not_emit_any_action() {
        let payload = json!({});
        assert!(hook_actions("SomeFutureEvent", &payload, "1").is_empty());
    }
}
