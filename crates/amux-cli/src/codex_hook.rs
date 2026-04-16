//! Codex CLI hook event handling.
//!
//! Reads JSON from stdin and translates Codex CLI hook events (SessionStart,
//! UserPromptSubmit, PreToolUse, PostToolUse, Stop) into amux IPC calls that
//! update workspace status.
//!
//! Codex currently exposes no hook event for approval prompts or "needs
//! input" transitions, so unlike Claude Code and Gemini CLI this integration
//! never emits `state: "waiting"` / `label: "Needs input"`. Revisit once
//! Codex adds a hook for approval requests.
//!
//! Per parity plan gap G23, tool-use events publish to a `codex.tool` key
//! in addition to the legacy `agent.message` dual-write. PostToolUse
//! removes the keyed entry without blanking the legacy message, which is
//! what closes the flicker between consecutive tool calls.

use crate::hook_action::{dispatch_actions, HookAction, TOOL_PRIORITY};
use amux_ipc::IpcClient;
use serde_json::{json, Value};
use std::io::Read as _;

/// Publisher-owned key for Codex per-tool entries.
pub(crate) const KEY_TOOL: &str = "codex.tool";

/// Pure helper: map a Codex hook event + payload to a list of
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
            HookAction::remove(KEY_TOOL),
        ],
        "UserPromptSubmit" => {
            let prompt = data.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
            let task = truncate(prompt, 80);
            let mut params = json!({
                "workspace_id": ws_id,
                "state": "active",
                "label": "Running",
                "message": "",
            });
            if !task.is_empty() {
                params["task"] = json!(task);
            }
            vec![HookAction::SetStatus(params), HookAction::remove(KEY_TOOL)]
        }
        "PreToolUse" => {
            let tool_name = data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
            let description = describe_tool_use(tool_name, data.get("tool_input"));
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
            // G23 flicker fix: only remove the keyed entry. Legacy
            // message persists so the sidebar doesn't blank between
            // consecutive tool calls in the same turn.
            vec![HookAction::remove(KEY_TOOL)]
        }
        "Stop" => vec![
            HookAction::SetStatus(json!({
                "workspace_id": ws_id,
                "state": "idle",
                "label": "Idle",
                "task": "",
                "message": "",
            })),
            HookAction::remove(KEY_TOOL),
        ],
        // Codex may add more events in the future; observe but don't react.
        _ => Vec::new(),
    }
}

/// Codex PreToolUse currently only intercepts the Bash tool per the docs.
/// Match on `tool_name` so future tool coverage fits without a rewrite.
fn describe_tool_use(tool_name: &str, tool_input: Option<&Value>) -> String {
    let null = Value::Null;
    let input = tool_input.unwrap_or(&null);
    match tool_name {
        "Bash" | "bash" => {
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if cmd.is_empty() {
                "Running command".to_string()
            } else {
                format!("Running {}", truncate(cmd, 60))
            }
        }
        "" => "Running".to_string(),
        other => other.to_string(),
    }
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

pub async fn handle_codex_hook(client: &mut IpcClient, event: &str) -> anyhow::Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let data: Value = serde_json::from_str(&input).unwrap_or_default();
    let ws_id = std::env::var("AMUX_WORKSPACE_ID").unwrap_or_else(|_| "0".to_string());

    let actions = hook_actions(event, &data, &ws_id);
    dispatch_actions(client, &ws_id, actions).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn set_status_params(actions: &[HookAction]) -> &Value {
        for a in actions {
            if let HookAction::SetStatus(v) = a {
                return v;
            }
        }
        panic!("no SetStatus action in {actions:?}");
    }

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

    fn removes(actions: &[HookAction]) -> Vec<&str> {
        actions
            .iter()
            .filter_map(|a| match a {
                HookAction::RemoveEntry { key } => Some(key.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn session_start_sets_idle_and_clears_key() {
        let payload = json!({ "hook_event_name": "SessionStart", "cwd": "/p" });
        let actions = hook_actions("SessionStart", &payload, "42");
        let params = set_status_params(&actions);
        assert_eq!(params["workspace_id"], "42");
        assert_eq!(params["state"], "idle");
        assert_eq!(params["label"], "Idle");
        assert_eq!(params["task"], "");
        assert_eq!(params["message"], "");
        assert_eq!(removes(&actions), vec!["codex.tool"]);
    }

    #[test]
    fn user_prompt_submit_sets_active_with_prompt() {
        let payload = json!({
            "hook_event_name": "UserPromptSubmit",
            "prompt": "refactor auth",
        });
        let actions = hook_actions("UserPromptSubmit", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "active");
        assert_eq!(params["label"], "Running");
        assert_eq!(params["task"], "refactor auth");
        assert_eq!(removes(&actions), vec!["codex.tool"]);
    }

    #[test]
    fn user_prompt_submit_without_prompt_emits_running_no_task() {
        let payload = json!({ "hook_event_name": "UserPromptSubmit" });
        let actions = hook_actions("UserPromptSubmit", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "active");
        assert!(params.get("task").is_none() || params["task"] == "");
    }

    #[test]
    fn user_prompt_submit_truncates_long_prompts_to_80_chars() {
        let long = "a".repeat(200);
        let payload = json!({ "prompt": long });
        let actions = hook_actions("UserPromptSubmit", &payload, "1");
        let params = set_status_params(&actions);
        let task = params["task"].as_str().unwrap();
        assert_eq!(task.chars().count(), 80);
        assert!(task.ends_with("..."));
    }

    #[test]
    fn pre_tool_use_bash_writes_legacy_and_keyed_entry() {
        // Codex PreToolUse currently only intercepts Bash per the docs.
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test" }
        });
        let actions = hook_actions("PreToolUse", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "active");
        assert_eq!(params["message"], "Running cargo test");
        assert_eq!(
            upserts(&actions),
            vec![("codex.tool", "Running cargo test", TOOL_PRIORITY)]
        );
    }

    #[test]
    fn pre_tool_use_truncates_long_command() {
        let payload = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "a".repeat(200) }
        });
        let actions = hook_actions("PreToolUse", &payload, "1");
        let params = set_status_params(&actions);
        let msg = params["message"].as_str().unwrap();
        assert!(msg.starts_with("Running "));
        assert!(msg.ends_with("..."));
    }

    #[test]
    fn pre_tool_use_missing_command_falls_back_to_generic_label() {
        let payload = json!({ "tool_name": "Bash", "tool_input": {} });
        let actions = hook_actions("PreToolUse", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["message"], "Running command");
    }

    /// G23 flicker fix: PostToolUse must NOT emit a `status.set` that
    /// blanks the legacy message. It only expires the keyed entry.
    #[test]
    fn post_tool_use_only_removes_keyed_entry() {
        let payload = json!({ "hook_event_name": "PostToolUse" });
        let actions = hook_actions("PostToolUse", &payload, "1");
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, HookAction::SetStatus(_))),
            "PostToolUse must not emit status.set"
        );
        assert_eq!(removes(&actions), vec!["codex.tool"]);
    }

    #[test]
    fn stop_goes_idle_and_clears() {
        let payload = json!({});
        let actions = hook_actions("Stop", &payload, "9");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "idle");
        assert_eq!(params["label"], "Idle");
        assert_eq!(params["task"], "");
        assert_eq!(params["message"], "");
        assert_eq!(removes(&actions), vec!["codex.tool"]);
    }

    #[test]
    fn unknown_event_does_not_emit_any_action() {
        let payload = json!({});
        assert!(hook_actions("Notification", &payload, "1").is_empty());
    }
}
