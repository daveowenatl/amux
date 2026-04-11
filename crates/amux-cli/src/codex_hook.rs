//! Codex CLI hook event handling.
//!
//! Reads JSON from stdin and translates Codex CLI hook events (SessionStart,
//! UserPromptSubmit, PreToolUse, PostToolUse, Stop) into amux IPC calls that
//! update workspace status.

use amux_ipc::IpcClient;
use serde_json::Value;

/// Pure helper: map a Codex hook event + payload to the status.set params
/// the IPC layer expects. Returns None when the event should be observed
/// but produces no status change.
pub(crate) fn status_update_for(event: &str, data: &Value, ws_id: &str) -> Option<Value> {
    let _ = (event, data, ws_id);
    unimplemented!("status_update_for — implemented in Task 2")
}

pub async fn handle_codex_hook(_client: &mut IpcClient, _event: &str) -> anyhow::Result<()> {
    unimplemented!("handle_codex_hook — implemented in Task 2")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn session_start_sets_idle() {
        let payload = json!({
            "session_id": "s1",
            "hook_event_name": "SessionStart",
            "cwd": "/Users/me/proj",
        });
        let params = status_update_for("SessionStart", &payload, "42").expect("emit");
        assert_eq!(params["workspace_id"], "42");
        assert_eq!(params["state"], "idle");
    }
}
