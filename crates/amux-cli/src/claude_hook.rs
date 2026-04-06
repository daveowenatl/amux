//! Claude Code hook event handling.
//!
//! Reads JSON from stdin and translates Claude Code hook events
//! (PreToolUse, PostToolUse, Stop, etc.) into amux IPC calls that
//! update workspace status, send notifications, and manage agent state.

use amux_ipc::IpcClient;
use std::io::Read as _;

pub async fn handle_claude_hook(client: &mut IpcClient, event: &str) -> anyhow::Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let data: serde_json::Value = serde_json::from_str(&input).unwrap_or_default();

    let ws_id = std::env::var("AMUX_WORKSPACE_ID").unwrap_or_else(|_| "0".to_string());

    match event {
        "UserPromptSubmit" => {
            // User submitted a prompt — set status to active with the prompt as task
            let prompt = data
                .get("prompt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let task = if prompt.len() > 80 {
                format!("{}...", &prompt[..77])
            } else {
                prompt
            };
            let mut params = serde_json::json!({
                "workspace_id": ws_id,
                "state": "active",
                "label": "Running",
            });
            if !task.is_empty() {
                params["task"] = serde_json::json!(task);
            }
            // Clear previous message on new prompt
            params["message"] = serde_json::json!("");
            client.call("status.set", params).await?;
        }
        "PreToolUse" => {
            // Claude is about to use a tool — update message with tool description
            let tool_name = data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
            let tool_input = data.get("tool_input");
            let description = describe_tool_use(tool_name, tool_input);
            let params = serde_json::json!({
                "workspace_id": ws_id,
                "state": "active",
                "label": "Running",
                "message": description,
            });
            client.call("status.set", params).await?;
        }
        "Notification" => {
            // Claude needs attention (permission prompt, etc.)
            let params = serde_json::json!({
                "workspace_id": ws_id,
                "state": "waiting",
                "label": "Needs input",
            });
            client.call("status.set", params).await?;
        }
        "Stop" => {
            // Claude finished its turn — set to idle, clear task/message
            let params = serde_json::json!({
                "workspace_id": ws_id,
                "state": "idle",
                "label": "Idle",
                "task": "",
                "message": "",
            });
            client.call("status.set", params).await?;
        }
        "SubagentStart" => {
            let agent_name = data
                .get("agent_name")
                .and_then(|v| v.as_str())
                .unwrap_or("subagent");
            let description = format!("Running {agent_name}");
            let params = serde_json::json!({
                "workspace_id": ws_id,
                "state": "active",
                "label": "Running",
                "message": description,
            });
            client.call("status.set", params).await?;
        }
        _ => {
            // SessionStart, SessionEnd, PostToolUse, SubagentStop — no status change needed
        }
    }

    Ok(())
}

/// Generate a human-readable description of a tool use, matching cmux's describeToolUse().
fn describe_tool_use(tool_name: &str, tool_input: Option<&serde_json::Value>) -> String {
    let input = tool_input.unwrap_or(&serde_json::Value::Null);

    match tool_name {
        "Read" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let filename = std::path::Path::new(path)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(path);
            format!("Reading {filename}")
        }
        "Edit" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let filename = std::path::Path::new(path)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(path);
            format!("Editing {filename}")
        }
        "Write" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let filename = std::path::Path::new(path)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or(path);
            format!("Writing {filename}")
        }
        "Bash" => {
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let short = if cmd.len() > 60 {
                format!("{}...", &cmd[..57])
            } else {
                cmd.to_string()
            };
            format!("Running {short}")
        }
        "Glob" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("*");
            format!("Searching {pattern}")
        }
        "Grep" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            format!("Grep {pattern}")
        }
        "WebFetch" => "Fetching URL".to_string(),
        "WebSearch" => {
            let query = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
            format!("Search: {query}")
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
