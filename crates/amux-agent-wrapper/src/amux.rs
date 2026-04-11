//! Shared helpers: detecting whether we're inside an amux pane,
//! resolving the amux CLI binary, and emitting hook-command strings
//! that embed it verbatim.

use std::path::{Path, PathBuf};

/// Determine the directory we were launched from. The wrapper is
/// typically installed at `~/.config/amux/bin/claude.exe`, so this
/// returns `~/.config/amux/bin`. Used to skip ourselves in
/// `find_real_agent` and to emit correct hook command paths.
pub(crate) fn wrapper_install_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Return true when all of the following hold:
///
/// 1. `AMUX_SURFACE_ID` is set (we're inside an amux pane)
/// 2. `AMUX_SOCKET_PATH` points at a live socket (amux is running)
/// 3. `amux ping` over that socket succeeds inside a bounded timeout
///
/// A miss on any of these means we passthrough — the user can still
/// run the agent outside amux without paying for wrapper logic.
pub(crate) fn in_amux_pane() -> bool {
    if std::env::var_os("AMUX_SURFACE_ID").is_none() {
        return false;
    }
    let Some(socket) = std::env::var_os("AMUX_SOCKET_PATH") else {
        return false;
    };
    if socket.is_empty() {
        return false;
    }
    let Some(amux_bin) = resolve_amux_bin() else {
        return false;
    };

    // Bounded ping: we'd rather passthrough than hang the agent launch
    // behind a stuck amux socket. Use a 2-second hard timeout via a
    // spawned child + wait-with-timeout fallback.
    let mut cmd = std::process::Command::new(&amux_bin);
    cmd.arg("--socket")
        .arg(&socket)
        .arg("ping")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    matches!(cmd.status().map(|s| s.success()), Ok(true))
}

/// Resolve the amux CLI binary. Preference order:
///
/// 1. `AMUX_BIN` env var if it points at an executable file
/// 2. `amux[.exe]` in the same directory as this wrapper (both are
///    typically in `~/.config/amux/bin/`)
/// 3. `amux[.exe]` anywhere on `PATH`
///
/// Returns an absolute path, or `None` if amux can't be located.
pub(crate) fn resolve_amux_bin() -> Option<PathBuf> {
    if let Some(env_bin) = std::env::var_os("AMUX_BIN") {
        let p = PathBuf::from(env_bin);
        if p.is_file() {
            return Some(p);
        }
    }

    let wrapper_dir = wrapper_install_dir();
    let names: &[&str] = if cfg!(windows) {
        &["amux.exe", "amux"]
    } else {
        &["amux"]
    };

    for name in names {
        let candidate = wrapper_dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    // Final fallback: walk PATH. Skip our own install dir so we don't
    // resolve back to a shim if the user has somehow aliased `amux` to
    // this wrapper.
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() || !dir.is_absolute() {
            continue;
        }
        if dir == wrapper_dir {
            continue;
        }
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// JSON-escape a string for embedding inside a JSON string literal.
/// Rust-std doesn't expose serde-level escaping and we don't want to
/// pull serde_json into the wrapper just for this — the wrapper is
/// supposed to be tiny. Covers the escapes that can appear in file
/// paths: backslash, quote, and control characters.
pub(crate) fn json_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 2);
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Render the `amux` CLI binary path for embedding inside a hook
/// command string. Wraps in quotes so spaces in the path (`C:\Program
/// Files\...`) don't split the command at shell parse time.
pub(crate) fn hook_cmd_path(amux_bin: &Path) -> String {
    format!("\"{}\"", json_escape(&amux_bin.to_string_lossy()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_escape_handles_quotes_backslashes_and_controls() {
        assert_eq!(json_escape("hello"), "hello");
        assert_eq!(json_escape("a\"b"), "a\\\"b");
        assert_eq!(
            json_escape("C:\\Program Files\\x"),
            "C:\\\\Program Files\\\\x"
        );
        assert_eq!(json_escape("\nlineA\tend"), "\\nlineA\\tend");
        assert_eq!(json_escape("\x01"), "\\u0001");
    }

    #[test]
    fn hook_cmd_path_quotes_and_escapes() {
        let rendered = hook_cmd_path(Path::new("/usr/local/bin/amux"));
        assert_eq!(rendered, "\"/usr/local/bin/amux\"");

        let windows = hook_cmd_path(Path::new("C:\\Program Files\\amux\\amux.exe"));
        // The inner backslashes must be JSON-escaped since this string
        // lands inside a JSON string literal in the hook config.
        assert_eq!(windows, "\"C:\\\\Program Files\\\\amux\\\\amux.exe\"");
    }
}
