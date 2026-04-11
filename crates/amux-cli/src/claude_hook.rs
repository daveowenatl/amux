//! Claude Code hook event handling.
//!
//! Reads JSON from stdin and translates Claude Code hook events
//! (PreToolUse, PostToolUse, Stop, etc.) into amux IPC calls that
//! update workspace status, send notifications, and manage agent state.

use amux_ipc::IpcClient;
use serde_json::{json, Value};
use std::io::Read as _;

/// Pure helper: map a Claude Code hook event + payload to the status.set
/// params the IPC layer expects. Returns None when the event should be
/// observed but produces no status change.
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
        "PostToolUse" => {
            // Tool call finished. Keep "Running" but clear the per-tool
            // message so the next PreToolUse overwrites cleanly, and so a
            // stuck "Running cargo test" indicator can't linger if Claude
            // pauses between tool calls within the same turn.
            Some(json!({
                "workspace_id": ws_id,
                "state": "active",
                "label": "Running",
                "message": "",
            }))
        }
        "Notification" => {
            // Claude needs attention. Claude's Notification payload includes
            // a `message` field with context-specific text (permission prompt,
            // idle warning, etc.) — surface it rather than the generic label.
            //
            // Always set the `message` field explicitly (defaulting to "" when
            // the payload omits it) so set_status clears any stale per-tool
            // message from a prior PreToolUse. Passing `None` would *preserve*
            // the previous message, leaving the user looking at "Running cargo
            // test" while the state has already flipped to "waiting".
            let message = data.get("message").and_then(|v| v.as_str()).unwrap_or("");
            Some(json!({
                "workspace_id": ws_id,
                "state": "waiting",
                "label": "Needs input",
                "message": message,
            }))
        }
        "Stop" => Some(json!({
            "workspace_id": ws_id,
            "state": "idle",
            "label": "Idle",
            "task": "",
            "message": "",
        })),
        "SubagentStart" => {
            let agent_name = data
                .get("agent_name")
                .and_then(|v| v.as_str())
                .unwrap_or("subagent");
            Some(json!({
                "workspace_id": ws_id,
                "state": "active",
                "label": "Running",
                "message": format!("Running {agent_name}"),
            }))
        }
        "SubagentStop" => {
            // Subagent finished, but the parent agent is probably still
            // running. Clear the subagent message; parent hook events will
            // replace it on the next tool call or Stop.
            Some(json!({
                "workspace_id": ws_id,
                "state": "active",
                "label": "Running",
                "message": "",
            }))
        }
        "SessionEnd" => Some(json!({
            "workspace_id": ws_id,
            "state": "idle",
            "label": "Idle",
            "task": "",
            "message": "",
        })),
        _ => None,
    }
}

pub async fn handle_claude_hook(client: &mut IpcClient, event: &str) -> anyhow::Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let data: Value = serde_json::from_str(&input).unwrap_or_default();
    let ws_id = std::env::var("AMUX_WORKSPACE_ID").unwrap_or_else(|_| "0".to_string());

    if let Some(params) = status_update_for(event, &data, &ws_id) {
        client.call("status.set", params).await?;
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

    // ---- SessionStart / SessionEnd ----

    #[test]
    fn session_start_sets_idle() {
        let payload = json!({ "hook_event_name": "SessionStart" });
        let params = status_update_for("SessionStart", &payload, "42").unwrap();
        assert_eq!(params["workspace_id"], "42");
        assert_eq!(params["state"], "idle");
        assert_eq!(params["label"], "Idle");
        assert_eq!(params["task"], "");
        assert_eq!(params["message"], "");
    }

    #[test]
    fn session_end_sets_idle_and_clears() {
        let payload = json!({ "hook_event_name": "SessionEnd" });
        let params = status_update_for("SessionEnd", &payload, "1").unwrap();
        assert_eq!(params["state"], "idle");
        assert_eq!(params["task"], "");
        assert_eq!(params["message"], "");
    }

    // ---- UserPromptSubmit ----

    #[test]
    fn user_prompt_submit_sets_active_with_prompt() {
        let payload = json!({ "prompt": "refactor the auth module" });
        let params = status_update_for("UserPromptSubmit", &payload, "42").unwrap();
        assert_eq!(params["state"], "active");
        assert_eq!(params["label"], "Running");
        assert_eq!(params["task"], "refactor the auth module");
    }

    #[test]
    fn user_prompt_submit_without_prompt_omits_task() {
        let payload = json!({});
        let params = status_update_for("UserPromptSubmit", &payload, "1").unwrap();
        assert_eq!(params["state"], "active");
        assert!(params.get("task").is_none() || params["task"] == "");
    }

    #[test]
    fn user_prompt_submit_truncates_long_prompts() {
        let long = "a".repeat(200);
        let payload = json!({ "prompt": long });
        let params = status_update_for("UserPromptSubmit", &payload, "1").unwrap();
        let task = params["task"].as_str().unwrap();
        assert_eq!(task.chars().count(), 80);
        assert!(task.ends_with("..."));
    }

    // ---- PreToolUse ----

    #[test]
    fn pre_tool_use_bash() {
        let payload = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "cargo test" }
        });
        let params = status_update_for("PreToolUse", &payload, "1").unwrap();
        assert_eq!(params["state"], "active");
        assert_eq!(params["message"], "Running cargo test");
    }

    #[test]
    fn pre_tool_use_read_shows_filename() {
        let payload = json!({
            "tool_name": "Read",
            "tool_input": { "file_path": "/Users/me/project/src/main.rs" }
        });
        let params = status_update_for("PreToolUse", &payload, "1").unwrap();
        assert_eq!(params["message"], "Reading main.rs");
    }

    #[test]
    fn pre_tool_use_missing_path_falls_back_to_generic_label() {
        let payload = json!({ "tool_name": "Read", "tool_input": {} });
        let params = status_update_for("PreToolUse", &payload, "1").unwrap();
        assert_eq!(params["message"], "Reading file");

        let payload = json!({ "tool_name": "Bash", "tool_input": {} });
        let params = status_update_for("PreToolUse", &payload, "1").unwrap();
        assert_eq!(params["message"], "Running command");
    }

    // ---- PostToolUse ----

    #[test]
    fn post_tool_use_clears_message_keeps_active_state() {
        let payload = json!({ "tool_name": "Bash", "tool_output": "..." });
        let params = status_update_for("PostToolUse", &payload, "1").unwrap();
        assert_eq!(params["state"], "active");
        assert_eq!(params["label"], "Running");
        assert_eq!(params["message"], "");
    }

    // ---- Notification ----

    #[test]
    fn notification_sets_waiting_and_surfaces_message() {
        let payload = json!({
            "message": "Claude needs your permission to run a command",
            "title": "Permission request"
        });
        let params = status_update_for("Notification", &payload, "1").unwrap();
        assert_eq!(params["state"], "waiting");
        assert_eq!(params["label"], "Needs input");
        assert_eq!(
            params["message"],
            "Claude needs your permission to run a command"
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
        let params = status_update_for("Notification", &payload, "1").unwrap();
        assert_eq!(params["state"], "waiting");
        assert_eq!(params["label"], "Needs input");
        assert_eq!(
            params["message"], "",
            "message must be explicit empty string, not absent, so set_status clears"
        );
    }

    // ---- Stop ----

    #[test]
    fn stop_goes_idle_and_clears() {
        let payload = json!({});
        let params = status_update_for("Stop", &payload, "9").unwrap();
        assert_eq!(params["state"], "idle");
        assert_eq!(params["task"], "");
        assert_eq!(params["message"], "");
    }

    // ---- SubagentStart / SubagentStop ----

    #[test]
    fn subagent_start_shows_agent_name() {
        let payload = json!({ "agent_name": "code-reviewer" });
        let params = status_update_for("SubagentStart", &payload, "1").unwrap();
        assert_eq!(params["state"], "active");
        assert_eq!(params["message"], "Running code-reviewer");
    }

    #[test]
    fn subagent_start_without_name_uses_default() {
        let payload = json!({});
        let params = status_update_for("SubagentStart", &payload, "1").unwrap();
        assert_eq!(params["message"], "Running subagent");
    }

    #[test]
    fn subagent_stop_clears_message_keeps_active() {
        let payload = json!({ "agent_name": "code-reviewer" });
        let params = status_update_for("SubagentStop", &payload, "1").unwrap();
        assert_eq!(params["state"], "active");
        assert_eq!(params["message"], "");
    }

    // ---- Unknown events ----

    #[test]
    fn unknown_event_does_not_emit_status_change() {
        let payload = json!({});
        assert!(status_update_for("SomeFutureEvent", &payload, "1").is_none());
    }
}
