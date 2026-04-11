//! Gemini CLI hook event handling.
//!
//! Reads JSON from stdin and translates Gemini CLI hook events
//! (BeforeTool, AfterTool, BeforeAgent, AfterAgent, Notification,
//! SessionStart, SessionEnd) into amux IPC calls that update workspace
//! status and send notifications.

use amux_ipc::IpcClient;
use serde_json::Value;

/// Pure helper: map a Gemini hook event + payload to the status.set params
/// the IPC layer expects. Returns None when the event should be observed
/// but produces no status change.
pub(crate) fn status_update_for(event: &str, data: &Value, ws_id: &str) -> Option<Value> {
    let _ = (event, data, ws_id);
    unimplemented!("status_update_for — implemented in Task 2")
}

pub async fn handle_gemini_hook(_client: &mut IpcClient, _event: &str) -> anyhow::Result<()> {
    // Thin wrapper over status_update_for — implemented in Task 2.
    unimplemented!("handle_gemini_hook — implemented in Task 2")
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
}
