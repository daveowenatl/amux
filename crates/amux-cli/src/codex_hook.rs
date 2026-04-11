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

use amux_ipc::IpcClient;
use serde_json::{json, Value};
use std::io::Read as _;

/// Pure helper: map a Codex hook event + payload to the status.set params
/// the IPC layer expects. Returns None when the event should be observed
/// but produces no status change.
pub(crate) fn status_update_for(event: &str, data: &Value, ws_id: &str) -> Option<Value> {
    match event {
        "SessionStart" => Some(json!({
            "workspace_id": ws_id,
            "state": "idle",
            "label": "Idle",
            "task": "",
            "message": "",
        })),
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
            Some(params)
        }
        "PreToolUse" => {
            let tool_name = data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
            let description = describe_tool_use(tool_name, data.get("tool_input"));
            Some(json!({
                "workspace_id": ws_id,
                "state": "active",
                "label": "Running",
                "message": description,
            }))
        }
        "PostToolUse" => Some(json!({
            "workspace_id": ws_id,
            "state": "active",
            "label": "Running",
            "message": "",
        })),
        "Stop" => Some(json!({
            "workspace_id": ws_id,
            "state": "idle",
            "label": "Idle",
            "task": "",
            "message": "",
        })),
        // Codex may add more events in the future; observe but don't react.
        _ => None,
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

    if let Some(params) = status_update_for(event, &data, &ws_id) {
        client.call("status.set", params).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_start_sets_idle() {
        let payload = json!({ "hook_event_name": "SessionStart", "cwd": "/p" });
        let params = status_update_for("SessionStart", &payload, "42").expect("emit");
        assert_eq!(params["workspace_id"], "42");
        assert_eq!(params["state"], "idle");
        assert_eq!(params["label"], "Idle");
        assert_eq!(params["task"], "");
        assert_eq!(params["message"], "");
    }

    #[test]
    fn user_prompt_submit_sets_active_with_prompt() {
        let payload = json!({
            "hook_event_name": "UserPromptSubmit",
            "prompt": "refactor auth",
        });
        let params = status_update_for("UserPromptSubmit", &payload, "1").unwrap();
        assert_eq!(params["state"], "active");
        assert_eq!(params["label"], "Running");
        assert_eq!(params["task"], "refactor auth");
    }

    #[test]
    fn user_prompt_submit_without_prompt_emits_running_no_task() {
        let payload = json!({ "hook_event_name": "UserPromptSubmit" });
        let params = status_update_for("UserPromptSubmit", &payload, "1").unwrap();
        assert_eq!(params["state"], "active");
        assert!(params.get("task").is_none() || params["task"] == "");
    }

    #[test]
    fn user_prompt_submit_truncates_long_prompts_to_80_chars() {
        let long = "a".repeat(200);
        let payload = json!({ "prompt": long });
        let params = status_update_for("UserPromptSubmit", &payload, "1").unwrap();
        let task = params["task"].as_str().unwrap();
        assert_eq!(task.chars().count(), 80);
        assert!(task.ends_with("..."));
    }

    #[test]
    fn pre_tool_use_bash_shows_command() {
        // Codex PreToolUse currently only intercepts Bash per the docs.
        let payload = json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test" }
        });
        let params = status_update_for("PreToolUse", &payload, "1").unwrap();
        assert_eq!(params["state"], "active");
        assert_eq!(params["message"], "Running cargo test");
    }

    #[test]
    fn pre_tool_use_truncates_long_command() {
        let payload = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "a".repeat(200) }
        });
        let params = status_update_for("PreToolUse", &payload, "1").unwrap();
        let msg = params["message"].as_str().unwrap();
        assert!(msg.starts_with("Running "));
        assert!(msg.ends_with("..."));
    }

    #[test]
    fn pre_tool_use_missing_command_falls_back_to_generic_label() {
        let payload = json!({ "tool_name": "Bash", "tool_input": {} });
        let params = status_update_for("PreToolUse", &payload, "1").unwrap();
        assert_eq!(params["message"], "Running command");
    }

    #[test]
    fn post_tool_use_clears_message_keeps_active_state() {
        // After a tool call finishes, go back to "Running" with no specific
        // message so a later tool call overwrites cleanly.
        let payload = json!({ "hook_event_name": "PostToolUse" });
        let params = status_update_for("PostToolUse", &payload, "1").unwrap();
        assert_eq!(params["state"], "active");
        assert_eq!(params["label"], "Running");
        assert_eq!(params["message"], "");
    }

    #[test]
    fn stop_goes_idle_and_clears() {
        let payload = json!({});
        let params = status_update_for("Stop", &payload, "9").unwrap();
        assert_eq!(params["state"], "idle");
        assert_eq!(params["label"], "Idle");
        assert_eq!(params["task"], "");
        assert_eq!(params["message"], "");
    }

    #[test]
    fn unknown_event_does_not_emit_status_change() {
        let payload = json!({});
        assert!(status_update_for("Notification", &payload, "1").is_none());
    }
}
