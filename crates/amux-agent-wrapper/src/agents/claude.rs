//! Claude Code wrapper — mirrors the logic in `resources/bin/claude`.
//!
//! When launched inside an amux pane, builds a `--settings` JSON blob
//! registering all 9 Claude hook events and execs the real `claude`
//! binary with that blob prepended to argv. The settings JSON contains
//! hook commands that call `amux claude-hook <event>`.
//!
//! When not in an amux pane (or the socket is down), passthrough exec.

use std::ffi::OsString;

use crate::amux;

pub(crate) fn run(forward_args: &[OsString]) -> Result<u8, String> {
    let wrapper_dir = amux::wrapper_install_dir();
    let real_claude = crate::find_real_agent("claude", &wrapper_dir)
        .ok_or_else(|| "claude not found in PATH".to_string())?;

    // Passthrough when not in amux or amux isn't responding.
    if !amux::in_amux_pane() {
        return Ok(crate::spawn_and_wait(&real_claude, forward_args));
    }

    // Resolve the amux CLI binary once so every hook entry in the
    // generated JSON embeds the same verified absolute path.
    let Some(amux_bin) = amux::resolve_amux_bin() else {
        // We're in amux but can't find amux itself — passthrough rather
        // than refuse to start claude. Shell integration may still run.
        return Ok(crate::spawn_and_wait(&real_claude, forward_args));
    };

    // Claude Code hook subcommands we wire up. Kept in sync with the
    // `resources/bin/claude` bash script that #173 expanded to cover
    // all 9 hook events.
    let events: &[&str] = &[
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "Notification",
        "Stop",
        "SubagentStart",
        "SubagentStop",
        "SessionEnd",
    ];
    let hooks_json = build_hooks_json(&amux_bin, events);

    // Prepend `--settings <blob>` to the user's argv. Claude Code
    // merges --settings additively with the user's ~/.claude/settings.json
    // so their own hooks still fire.
    //
    // CLAUDECODE is unset to avoid "nested session" detection — amux
    // panes are independent sessions even when the parent shell was
    // launched from Claude Code.
    let mut cmd = std::process::Command::new(&real_claude);
    cmd.env_remove("CLAUDECODE");
    cmd.arg("--settings");
    cmd.arg(&hooks_json);
    cmd.args(forward_args);

    match cmd.status() {
        Ok(status) => Ok(status
            .code()
            .and_then(|c| u8::try_from(c & 0xff).ok())
            .unwrap_or(1)),
        Err(e) => Err(format!(
            "amux-agent-wrapper: failed to spawn {}: {e}",
            real_claude.display()
        )),
    }
}

/// Build the `--settings` JSON blob that registers an `amux claude-hook
/// <event>` command for every event in `events`. Mirrors the HEREDOC
/// structure of `resources/bin/claude`.
fn build_hooks_json(amux_bin: &std::path::Path, events: &[&str]) -> String {
    let cmd_path = amux::hook_cmd_path(amux_bin);
    let mut out = String::from("{\"hooks\":{");
    for (i, event) in events.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "\"{event}\":[{{\"matcher\":\"\",\"hooks\":[{{\
             \"type\":\"command\",\
             \"command\":\"{cmd_path} claude-hook {event}\",\
             \"timeout\":5\
             }}]}}]"
        ));
    }
    out.push_str("}}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn build_hooks_json_registers_each_event() {
        let json = build_hooks_json(Path::new("/usr/local/bin/amux"), &["SessionStart", "Stop"]);
        // Must parse as valid JSON with the expected structure.
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        let hooks = parsed["hooks"].as_object().expect("hooks object");
        assert!(hooks.contains_key("SessionStart"));
        assert!(hooks.contains_key("Stop"));
        for event in ["SessionStart", "Stop"] {
            let command = hooks[event][0]["hooks"][0]["command"]
                .as_str()
                .expect("command string");
            let expected = format!("\"/usr/local/bin/amux\" claude-hook {event}");
            assert_eq!(command, expected);
        }
    }

    #[test]
    fn build_hooks_json_escapes_windows_paths() {
        let json = build_hooks_json(Path::new("C:\\Program Files\\amux\\amux.exe"), &["Stop"]);
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        let command = parsed["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .expect("command string");
        // After JSON decoding, the command field must contain the raw
        // Windows path (single backslashes) wrapped in shell quotes.
        assert_eq!(
            command,
            "\"C:\\Program Files\\amux\\amux.exe\" claude-hook Stop"
        );
    }

    #[test]
    fn build_hooks_json_empty_events_still_parses() {
        let json = build_hooks_json(Path::new("/usr/local/bin/amux"), &[]);
        assert_eq!(json, "{\"hooks\":{}}");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert!(parsed["hooks"].as_object().unwrap().is_empty());
    }
}
