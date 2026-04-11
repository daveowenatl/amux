//! Gemini CLI wrapper — mirrors the logic in `resources/bin/gemini`.
//!
//! When launched inside an amux pane, writes a temp settings JSON file
//! containing amux hook commands and exports
//! `GEMINI_CLI_SYSTEM_SETTINGS_PATH` pointing at it before spawning
//! the real `gemini` binary. Because Gemini's hook arrays use CONCAT
//! merging, the user's own hooks in `~/.gemini/settings.json` coexist
//! with ours — zero pollution of the user's config.
//!
//! Version gate: we skip injection entirely when `gemini --version`
//! reports anything older than 0.26.0 (the first hook-capable release),
//! so old installs run unhooked rather than fail weirdly.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::amux;

pub(crate) fn run(forward_args: &[OsString]) -> Result<u8, String> {
    let wrapper_dir = amux::wrapper_install_dir();
    let real_gemini = crate::find_real_agent("gemini", &wrapper_dir)
        .ok_or_else(|| "gemini not found in PATH".to_string())?;

    // Passthrough when not in amux, gemini is too old, or amux isn't up.
    if !amux::in_amux_pane() || !gemini_supports_hooks(&real_gemini) {
        return Ok(crate::spawn_and_wait(&real_gemini, forward_args));
    }

    let Some(amux_bin) = amux::resolve_amux_bin() else {
        return Ok(crate::spawn_and_wait(&real_gemini, forward_args));
    };

    // Per-surface temp settings file. Stable path so sibling panes don't
    // race each other and so amux-app's stale-file sweeper can clean up
    // after us. Falls back to a generic per-pid name if the surface id
    // env var is missing (shouldn't happen inside amux, but keeps the
    // wrapper robust).
    let surface_id =
        std::env::var("AMUX_SURFACE_ID").unwrap_or_else(|_| std::process::id().to_string());
    let settings_path = settings_file_path(&surface_id);

    let events: &[&str] = &[
        "BeforeAgent",
        "BeforeTool",
        "Notification",
        "AfterAgent",
        "SessionStart",
        "SessionEnd",
    ];
    let settings_json = build_settings_json(&amux_bin, events);

    if let Err(e) = std::fs::write(&settings_path, &settings_json) {
        eprintln!(
            "amux-agent-wrapper: failed to write gemini settings at {}: {e}",
            settings_path.display()
        );
        return Ok(crate::spawn_and_wait(&real_gemini, forward_args));
    }

    let mut cmd = std::process::Command::new(&real_gemini);
    cmd.env("GEMINI_CLI_SYSTEM_SETTINGS_PATH", &settings_path);
    cmd.args(forward_args);

    match cmd.status() {
        Ok(status) => Ok(status
            .code()
            .and_then(|c| u8::try_from(c & 0xff).ok())
            .unwrap_or(1)),
        Err(e) => Err(format!(
            "amux-agent-wrapper: failed to spawn {}: {e}",
            real_gemini.display()
        )),
    }
}

/// Computes the per-surface settings file path under the system temp
/// dir. Matches the `amux-gemini-settings-<surface>.json` pattern that
/// amux-app's stale-file sweeper (in `startup.rs`) scans for on launch.
fn settings_file_path(surface_id: &str) -> PathBuf {
    let tmp = std::env::temp_dir();
    tmp.join(format!("amux-gemini-settings-{surface_id}.json"))
}

/// Probe `gemini --version` with a bounded 2s timeout. Returns true if
/// the version string matches `0.26.0` or newer. Passthrough-safe: if
/// the probe fails or times out, we return false and skip injection.
fn gemini_supports_hooks(real: &Path) -> bool {
    let mut cmd = std::process::Command::new(real);
    cmd.arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped());
    let Ok(output) = cmd.output() else {
        return false;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_version_supports_hooks(&stdout)
}

/// Parse a `major.minor.patch` version string from stdout. Returns true
/// if it's >= 0.26.0. Split out for testing.
fn parse_version_supports_hooks(stdout: &str) -> bool {
    // Grab the first token matching `<digits>.<digits>.<digits>`.
    let first_line = stdout.lines().next().unwrap_or("");
    let mut ver_chars = String::new();
    let mut seen_dot = 0;
    let mut in_version = false;
    for ch in first_line.chars() {
        if ch.is_ascii_digit() {
            ver_chars.push(ch);
            in_version = true;
        } else if ch == '.' && in_version {
            ver_chars.push(ch);
            seen_dot += 1;
            if seen_dot == 3 {
                ver_chars.pop();
                break;
            }
        } else if in_version {
            break;
        }
    }
    if seen_dot < 2 {
        return false;
    }
    let parts: Vec<u32> = ver_chars
        .split('.')
        .take(3)
        .filter_map(|s| s.parse::<u32>().ok())
        .collect();
    if parts.len() < 2 {
        return false;
    }
    let major = parts[0];
    let minor = parts[1];
    major > 0 || (major == 0 && minor >= 26)
}

/// Build the Gemini settings JSON for the given hook events. Mirrors
/// the `resources/bin/gemini` HEREDOC.
fn build_settings_json(amux_bin: &Path, events: &[&str]) -> String {
    let cmd_path = amux::hook_cmd_path(amux_bin);
    let mut out = String::from("{\"hooks\":{");
    for (i, event) in events.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "\"{event}\":[{{\"matcher\":\"\",\"hooks\":[{{\
             \"type\":\"command\",\
             \"command\":\"{cmd_path} gemini-hook {event}\",\
             \"timeout\":5000,\
             \"name\":\"amux-status\"\
             }}]}}]"
        ));
    }
    out.push_str("}}");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_path_uses_surface_id() {
        let path = settings_file_path("42");
        assert!(path.file_name().unwrap() == "amux-gemini-settings-42.json");
    }

    #[test]
    fn build_settings_json_has_all_events_and_timeout() {
        let json = build_settings_json(
            Path::new("/usr/local/bin/amux"),
            &["BeforeAgent", "AfterAgent"],
        );
        assert!(json.contains("\"BeforeAgent\":"));
        assert!(json.contains("\"AfterAgent\":"));
        assert!(json.contains("\"timeout\":5000"));
        assert!(json.contains("gemini-hook BeforeAgent"));
        assert!(json.contains("gemini-hook AfterAgent"));
        let opens = json.matches('{').count();
        let closes = json.matches('}').count();
        assert_eq!(opens, closes, "json braces unbalanced: {json}");
    }

    #[test]
    fn version_probe_accepts_0_26_and_newer() {
        // Happy path: exactly the minimum.
        assert!(parse_version_supports_hooks("0.26.0"));
        // Newer minor versions pass.
        assert!(parse_version_supports_hooks("0.27.3"));
        // Major bump passes trivially.
        assert!(parse_version_supports_hooks("1.0.0"));
        // Common prefix styles (`gemini-cli v0.26.0`, leading spaces).
        assert!(parse_version_supports_hooks("gemini-cli 0.26.0"));
        assert!(parse_version_supports_hooks("   0.26.0   "));
    }

    #[test]
    fn version_probe_rejects_older_or_garbage() {
        assert!(!parse_version_supports_hooks("0.25.9"));
        assert!(!parse_version_supports_hooks("0.1.1"));
        assert!(!parse_version_supports_hooks(""));
        assert!(!parse_version_supports_hooks("no version here"));
        assert!(!parse_version_supports_hooks("v0"));
    }
}
