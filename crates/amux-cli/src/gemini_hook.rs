//! Gemini CLI hook event handling.
//!
//! Reads JSON from stdin and translates Gemini CLI hook events
//! (BeforeTool, AfterTool, BeforeAgent, AfterAgent, Notification,
//! SessionStart, SessionEnd) into amux IPC calls that update workspace
//! status and send notifications.
//!
//! Per parity plan gap G23, tool-use and notification events publish to
//! their own namespaced keys (`gemini.tool`, `gemini.notification`) via
//! `status.upsert_entry` in addition to the legacy `agent.message`
//! dual-write.

use crate::hook_action::{dispatch_actions, HookAction, NOTIFICATION_PRIORITY, TOOL_PRIORITY};
use amux_ipc::IpcClient;
use serde_json::{json, Value};
use std::io::Read as _;

/// Publisher-owned keys for Gemini hook emissions.
pub(crate) const KEY_TOOL: &str = "gemini.tool";
pub(crate) const KEY_NOTIFICATION: &str = "gemini.notification";

/// Pure helper: map a Gemini hook event + payload to a list of
/// [`HookAction`]s. Returns an empty Vec when the event is observed but
/// produces no status change (e.g., AfterTool, BeforeModel).
pub(crate) fn hook_actions(event: &str, data: &Value, ws_id: &str) -> Vec<HookAction> {
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
            vec![
                HookAction::SetStatus(params),
                // New agent turn: expire any lingering tool / notification
                // entries from the previous turn.
                HookAction::remove(KEY_TOOL),
                HookAction::remove(KEY_NOTIFICATION),
            ]
        }
        "BeforeTool" => {
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
        "AfterTool" => {
            // G23 flicker fix: no `status.set`, just expire the keyed
            // entry. Legacy message persists until the next BeforeTool
            // overwrites it — consecutive tools no longer blank in
            // between.
            vec![HookAction::remove(KEY_TOOL)]
        }
        "Notification" => {
            // Gemini's Notification hook payload often carries the text
            // under a `message` field, mirroring Claude's shape. If the
            // field is absent we still drive state=waiting; the keyed
            // entry becomes an empty string so the sidebar has something
            // to display when the legacy slot is stale.
            let message = data.get("message").and_then(|v| v.as_str()).unwrap_or("");
            vec![
                HookAction::SetStatus(json!({
                    "workspace_id": ws_id,
                    "state": "waiting",
                    "label": "Needs input",
                })),
                HookAction::upsert(KEY_NOTIFICATION, message, NOTIFICATION_PRIORITY),
            ]
        }
        "AfterAgent" | "SessionStart" | "SessionEnd" => vec![
            HookAction::SetStatus(json!({
                "workspace_id": ws_id,
                "state": "idle",
                "label": "Idle",
                "task": "",
                "message": "",
            })),
            HookAction::remove(KEY_TOOL),
            HookAction::remove(KEY_NOTIFICATION),
        ],
        // BeforeModel, AfterModel, BeforeToolSelection, PreCompress:
        // no status change, amux just observes.
        _ => Vec::new(),
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
            format_with_target("Reading", filename_of(path), "file")
        }
        "edit_file" | "edit" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format_with_target("Editing", filename_of(path), "file")
        }
        "write_file" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format_with_target("Writing", filename_of(path), "file")
        }
        "run_shell_command" => {
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            format_with_target("Running", &truncate(cmd, 60), "command")
        }
        "glob" => {
            let pat = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("*");
            format_with_target("Searching", pat, "files")
        }
        "search_file_content" | "grep" => {
            let pat = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            format_with_target("Grep", pat, "files")
        }
        "web_fetch" => "Fetching URL".to_string(),
        "google_web_search" => {
            let q = input.get("query").and_then(|v| v.as_str()).unwrap_or("");
            format_with_target("Search:", q, "web")
        }
        _ => tool_name.to_string(),
    }
}

/// Produce `"<verb> <target>"`, falling back to `"<verb> <fallback>"`
/// when the target is empty so we never render trailing whitespace like
/// `"Reading "`.
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

pub async fn handle_gemini_hook(client: &mut IpcClient, event: &str) -> anyhow::Result<()> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let data: Value = serde_json::from_str(&input).unwrap_or_else(|_| json!({}));
    let ws_id = std::env::var("AMUX_WORKSPACE_ID").unwrap_or_else(|_| "0".to_string());

    let actions = hook_actions(event, &data, &ws_id);
    dispatch_actions(client, &ws_id, actions).await?;

    // "Notification" means Gemini needs input. Deliver a stored
    // notification so pane ring, sidebar badge, and
    // auto_reorder_workspaces all fire.
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
                    "title": "Gemini CLI",
                    "body": message,
                }),
            )
            .await;
    }

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
    fn before_agent_sets_active_with_prompt_and_clears_prior_keys() {
        let payload = json!({
            "session_id": "s1",
            "hook_event_name": "BeforeAgent",
            "prompt": "refactor the auth module",
        });
        let actions = hook_actions("BeforeAgent", &payload, "42");
        let params = set_status_params(&actions);
        assert_eq!(params["workspace_id"], "42");
        assert_eq!(params["state"], "active");
        assert_eq!(params["label"], "Running");
        assert_eq!(params["task"], "refactor the auth module");
        let mut r = removes(&actions);
        r.sort();
        assert_eq!(r, vec!["gemini.notification", "gemini.tool"]);
    }

    #[test]
    fn before_agent_without_prompt_emits_running_no_task() {
        let payload = json!({ "session_id": "s1", "hook_event_name": "BeforeAgent" });
        let actions = hook_actions("BeforeAgent", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "active");
        assert_eq!(params["label"], "Running");
        assert!(params.get("task").is_none() || params["task"] == "");
    }

    #[test]
    fn before_agent_truncates_long_prompts_to_80_chars() {
        let long = "a".repeat(200);
        let payload = json!({ "prompt": long });
        let actions = hook_actions("BeforeAgent", &payload, "1");
        let params = set_status_params(&actions);
        let task = params["task"].as_str().unwrap();
        assert_eq!(task.chars().count(), 80);
        assert!(task.ends_with("..."));
    }

    #[test]
    fn before_tool_writes_legacy_message_and_keyed_entry() {
        let payload = json!({
            "tool_name": "run_shell_command",
            "tool_input": { "command": "cargo test" }
        });
        let actions = hook_actions("BeforeTool", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "active");
        assert_eq!(params["message"], "Running cargo test");
        assert_eq!(
            upserts(&actions),
            vec![("gemini.tool", "Running cargo test", TOOL_PRIORITY)]
        );
    }

    #[test]
    fn before_tool_edit_file_reports_filename_only() {
        let payload = json!({
            "tool_name": "edit_file",
            "tool_input": { "file_path": "/Users/me/project/src/main.rs" }
        });
        let actions = hook_actions("BeforeTool", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["message"], "Editing main.rs");
        assert_eq!(
            upserts(&actions),
            vec![("gemini.tool", "Editing main.rs", TOOL_PRIORITY)]
        );
    }

    #[test]
    fn before_tool_missing_filename_falls_back_to_generic_label() {
        let payload = json!({ "tool_name": "read_file", "tool_input": {} });
        let actions = hook_actions("BeforeTool", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["message"], "Reading file");

        let payload = json!({ "tool_name": "edit_file", "tool_input": { "file_path": "" } });
        let actions = hook_actions("BeforeTool", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["message"], "Editing file");

        let payload = json!({ "tool_name": "run_shell_command", "tool_input": {} });
        let actions = hook_actions("BeforeTool", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["message"], "Running command");
    }

    /// G23 flicker fix: AfterTool must not emit a `status.set` that would
    /// blank the legacy agent.message. It only expires the keyed entry.
    #[test]
    fn after_tool_only_removes_keyed_entry() {
        let payload = json!({ "tool_name": "edit_file" });
        let actions = hook_actions("AfterTool", &payload, "1");
        assert!(
            !actions
                .iter()
                .any(|a| matches!(a, HookAction::SetStatus(_))),
            "AfterTool must not emit status.set"
        );
        assert_eq!(removes(&actions), vec!["gemini.tool"]);
    }

    #[test]
    fn after_agent_goes_idle_and_clears_keys() {
        let payload = json!({});
        let actions = hook_actions("AfterAgent", &payload, "9");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "idle");
        assert_eq!(params["label"], "Idle");
        assert_eq!(params["task"], "");
        assert_eq!(params["message"], "");
        let mut r = removes(&actions);
        r.sort();
        assert_eq!(r, vec!["gemini.notification", "gemini.tool"]);
    }

    #[test]
    fn notification_event_sets_waiting_and_publishes_keyed_entry() {
        let payload = json!({ "message": "Need approval" });
        let actions = hook_actions("Notification", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "waiting");
        assert_eq!(params["label"], "Needs input");
        assert_eq!(
            upserts(&actions),
            vec![(
                "gemini.notification",
                "Need approval",
                NOTIFICATION_PRIORITY
            )]
        );
    }

    #[test]
    fn session_start_resets_to_idle_and_clears_keys() {
        let payload = json!({});
        let actions = hook_actions("SessionStart", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "idle");
        let mut r = removes(&actions);
        r.sort();
        assert_eq!(r, vec!["gemini.notification", "gemini.tool"]);
    }

    #[test]
    fn session_end_clears_status() {
        let payload = json!({});
        let actions = hook_actions("SessionEnd", &payload, "1");
        let params = set_status_params(&actions);
        assert_eq!(params["state"], "idle");
        assert_eq!(params["task"], "");
    }

    #[test]
    fn unknown_event_does_not_emit_any_action() {
        let payload = json!({});
        assert!(hook_actions("BeforeModel", &payload, "1").is_empty());
    }
}
