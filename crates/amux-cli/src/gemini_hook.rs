//! Gemini CLI hook event handling.
//!
//! Reads JSON from stdin and translates Gemini CLI hook events
//! (BeforeTool, AfterTool, BeforeAgent, AfterAgent, Notification,
//! SessionStart, SessionEnd) into amux IPC calls that update workspace
//! status and send notifications.

use amux_ipc::IpcClient;
use serde_json::{json, Value};
use std::io::Read as _;

/// Pure helper: map a Gemini hook event + payload to the status.set params
/// the IPC layer expects. Returns None when the event should be observed
/// but produces no status change (e.g., AfterTool, BeforeModel).
pub(crate) fn status_update_for(event: &str, data: &Value, ws_id: &str) -> Option<Value> {
    match event {
        "BeforeAgent" => {
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
        "BeforeTool" => {
            let tool_name = data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
            let description = describe_tool_use(tool_name, data.get("tool_input"));
            Some(json!({
                "workspace_id": ws_id,
                "state": "active",
                "label": "Running",
                "message": description,
            }))
        }
        "Notification" => Some(json!({
            "workspace_id": ws_id,
            "state": "waiting",
            "label": "Needs input",
        })),
        "AfterAgent" | "SessionStart" | "SessionEnd" => Some(json!({
            "workspace_id": ws_id,
            "state": "idle",
            "label": "Idle",
            "task": "",
            "message": "",
        })),
        // AfterTool, BeforeModel, AfterModel, BeforeToolSelection, PreCompress:
        // no status change, amux just observes.
        _ => None,
    }
}

/// Map Gemini tool names + inputs to a short human-readable status string.
/// Tool names differ from Claude's; see packages/core/src/tools/ in the
/// google-gemini/gemini-cli repo.
fn describe_tool_use(tool_name: &str, tool_input: Option<&Value>) -> String {
    let null = Value::Null;
    let input = tool_input.unwrap_or(&null);
    match tool_name {
        "read_file" => {
            let path = input
                .get("file_path")
                .or_else(|| input.get("absolute_path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("Reading {}", filename_of(path))
        }
        "edit_file" | "edit" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("Editing {}", filename_of(path))
        }
        "write_file" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("Writing {}", filename_of(path))
        }
        "run_shell_command" => {
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            format!("Running {}", truncate(cmd, 60))
        }
        "glob" => {
            let pat = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("*");
            format!("Searching {pat}")
        }
        "search_file_content" | "grep" => {
            let pat = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            format!("Grep {pat}")
        }
        "web_fetch" => "Fetching URL".to_string(),
        "google_web_search" => {
            let q = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
            format!("Search: {q}")
        }
        _ => tool_name.to_string(),
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

pub async fn handle_gemini_hook(client: &mut IpcClient, event: &str) -> anyhow::Result<()> {
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
    fn before_agent_sets_active_with_prompt() {
        let payload = json!({
            "session_id": "s1",
            "hook_event_name": "BeforeAgent",
            "prompt": "refactor the auth module",
        });
        let params = status_update_for("BeforeAgent", &payload, "42").expect("should emit");
        assert_eq!(params["workspace_id"], "42");
        assert_eq!(params["state"], "active");
        assert_eq!(params["label"], "Running");
        assert_eq!(params["task"], "refactor the auth module");
    }

    #[test]
    fn before_agent_without_prompt_emits_running_no_task() {
        let payload = json!({ "session_id": "s1", "hook_event_name": "BeforeAgent" });
        let params = status_update_for("BeforeAgent", &payload, "1").expect("emit");
        assert_eq!(params["state"], "active");
        assert_eq!(params["label"], "Running");
        assert!(params.get("task").is_none() || params["task"] == "");
    }

    #[test]
    fn before_agent_truncates_long_prompts_to_80_chars() {
        let long = "a".repeat(200);
        let payload = json!({ "prompt": long });
        let params = status_update_for("BeforeAgent", &payload, "1").unwrap();
        let task = params["task"].as_str().unwrap();
        assert_eq!(task.chars().count(), 80);
        assert!(task.ends_with("..."));
    }

    #[test]
    fn before_tool_sets_running_message_from_tool_name() {
        let payload = json!({
            "tool_name": "run_shell_command",
            "tool_input": { "command": "cargo test" }
        });
        let params = status_update_for("BeforeTool", &payload, "1").unwrap();
        assert_eq!(params["state"], "active");
        assert_eq!(params["message"], "Running cargo test");
    }

    #[test]
    fn before_tool_edit_file_reports_filename_only() {
        let payload = json!({
            "tool_name": "edit_file",
            "tool_input": { "file_path": "/Users/me/project/src/main.rs" }
        });
        let params = status_update_for("BeforeTool", &payload, "1").unwrap();
        assert_eq!(params["message"], "Editing main.rs");
    }

    #[test]
    fn after_agent_goes_idle_and_clears_task() {
        let payload = json!({});
        let params = status_update_for("AfterAgent", &payload, "9").unwrap();
        assert_eq!(params["state"], "idle");
        assert_eq!(params["label"], "Idle");
        assert_eq!(params["task"], "");
        assert_eq!(params["message"], "");
    }

    #[test]
    fn notification_event_sets_waiting_state() {
        let payload = json!({ "notification_type": "ToolPermission" });
        let params = status_update_for("Notification", &payload, "1").unwrap();
        assert_eq!(params["state"], "waiting");
        assert_eq!(params["label"], "Needs input");
    }

    #[test]
    fn session_start_resets_to_idle() {
        let payload = json!({});
        let params = status_update_for("SessionStart", &payload, "1").unwrap();
        assert_eq!(params["state"], "idle");
    }

    #[test]
    fn session_end_clears_status() {
        let payload = json!({});
        let params = status_update_for("SessionEnd", &payload, "1").unwrap();
        assert_eq!(params["state"], "idle");
        assert_eq!(params["task"], "");
    }

    #[test]
    fn after_tool_does_not_emit_status_change() {
        let payload = json!({ "tool_name": "edit_file" });
        assert!(status_update_for("AfterTool", &payload, "1").is_none());
    }

    #[test]
    fn unknown_event_does_not_emit_status_change() {
        let payload = json!({});
        assert!(status_update_for("BeforeModel", &payload, "1").is_none());
    }
}
