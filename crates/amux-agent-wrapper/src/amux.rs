//! Shared helpers: detecting whether we're inside an amux pane,
//! resolving the amux CLI binary, and emitting hook-command strings
//! that embed it verbatim.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// Poll `child` with a 50ms interval until it exits or `timeout`
/// elapses. On timeout the child is killed and reaped, and `None` is
/// returned. Implemented with `try_wait` + sleep instead of pulling in
/// the `wait-timeout` crate — keeps the wrapper dep-free, and 50ms is
/// short enough that realistic local IPC / version probes finish on
/// the first or second wakeup.
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Spawn `cmd`, wait up to `timeout`, and return whether the command
/// exited successfully. Used for fire-and-forget probes like
/// `amux … ping` where stdout is irrelevant.
pub(crate) fn run_status_with_timeout(mut cmd: Command, timeout: Duration) -> bool {
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    wait_with_timeout(&mut child, timeout)
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Spawn `cmd` with stdout captured, wait up to `timeout`, and return
/// the captured stdout on successful exit. Returns `None` on spawn
/// failure, non-zero exit, or timeout. Used for the `gemini --version`
/// probe.
pub(crate) fn run_capture_stdout_with_timeout(
    mut cmd: Command,
    timeout: Duration,
) -> Option<Vec<u8>> {
    cmd.stdout(std::process::Stdio::piped());
    let mut child = cmd.spawn().ok()?;
    let status = wait_with_timeout(&mut child, timeout)?;
    if !status.success() {
        return None;
    }
    // Child has already exited; read any buffered stdout directly. We
    // avoid `wait_with_output` because it takes ownership of the child
    // and we've already waited above.
    use std::io::Read as _;
    let mut buf = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        out.read_to_end(&mut buf).ok()?;
    }
    Some(buf)
}

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
    // behind a stuck amux socket. 2-second hard deadline enforced via
    // spawn + try_wait poll loop (see `run_status_with_timeout`).
    let mut cmd = Command::new(&amux_bin);
    cmd.arg("--socket")
        .arg(&socket)
        .arg("ping")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    run_status_with_timeout(cmd, Duration::from_secs(2))
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

/// Render the `amux` CLI binary path for embedding inside a JSON
/// string literal that represents a shell command. The returned string
/// is *not* a standalone JSON value — it's the inner contents of a
/// `"command":"…"` string literal.
///
/// Two levels of escaping are happening simultaneously:
/// 1. **Shell level**: we want the shell (which eventually runs the
///    hook command) to see `"C:\Program Files\amux\amux.exe"` with
///    literal quote characters so the path is parsed as one token.
/// 2. **JSON level**: those literal quote characters and any
///    backslashes must be escaped so the surrounding JSON string
///    parses correctly. A backslash becomes `\\` and a quote becomes
///    `\"` inside a JSON string literal.
///
/// So the return value for `C:\Program Files\amux\amux.exe` is
/// `\"C:\\Program Files\\amux\\amux.exe\"` (bytes: `\ " C : \ \ P … \ \ . e x e \ "`),
/// which when dropped into `"command":"<here> gemini-hook Evt"` yields
/// valid JSON that decodes to `"C:\Program Files\amux\amux.exe" gemini-hook Evt`.
pub(crate) fn hook_cmd_path(amux_bin: &Path) -> String {
    let escaped = json_escape(&amux_bin.to_string_lossy());
    format!("\\\"{escaped}\\\"")
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
        // Unix path: the outer quotes are JSON-escaped (`\"`) so they
        // survive embedding inside a JSON string literal and then get
        // seen by the shell as actual quote characters.
        let rendered = hook_cmd_path(Path::new("/usr/local/bin/amux"));
        assert_eq!(rendered, "\\\"/usr/local/bin/amux\\\"");

        // Windows path: both the outer quotes and every inner backslash
        // must be JSON-escaped.
        let windows = hook_cmd_path(Path::new("C:\\Program Files\\amux\\amux.exe"));
        assert_eq!(windows, "\\\"C:\\\\Program Files\\\\amux\\\\amux.exe\\\"");
    }

    /// Round-trip a `"command":"<hook_cmd_path … args>"` fragment
    /// through `serde_json` to confirm the escaping is correct. Guards
    /// against regressions where the outer quotes are accidentally
    /// emitted unescaped, which would silently produce malformed JSON
    /// that only breaks at agent launch time on a real system.
    #[test]
    fn hook_cmd_path_round_trips_inside_json_string() {
        for raw in [
            "/usr/local/bin/amux",
            "C:\\Program Files\\amux\\amux.exe",
            "/home/dave/bin with spaces/amux",
            "C:\\Users\\d\"o\"e\\amux.exe",
        ] {
            let fragment = hook_cmd_path(Path::new(raw));
            let json = format!("{{\"command\":\"{fragment} hook Evt\"}}");
            let parsed: serde_json::Value = serde_json::from_str(&json).unwrap_or_else(|e| {
                panic!("bad json for {raw}: {e}\nfragment={fragment}\njson={json}")
            });
            let command = parsed["command"].as_str().expect("string");
            // The decoded shell command must contain the raw path in
            // quotes followed by the rest of the args.
            let expected = format!("\"{raw}\" hook Evt");
            assert_eq!(command, expected, "round-trip mismatch for {raw}");
        }
    }

    /// Smoke test: `run_status_with_timeout` must actually enforce the
    /// deadline rather than block on a long-running child. Uses a
    /// subprocess that sleeps well past the 300ms deadline.
    #[cfg(unix)]
    #[test]
    fn run_status_with_timeout_kills_hung_child() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 10"]);
        let start = Instant::now();
        let ok = run_status_with_timeout(cmd, Duration::from_millis(300));
        let elapsed = start.elapsed();
        assert!(!ok, "hung child should not report success");
        assert!(
            elapsed < Duration::from_secs(2),
            "timeout not enforced: took {elapsed:?}"
        );
    }

    /// `run_status_with_timeout` returns true for a command that
    /// completes cleanly inside the deadline.
    #[cfg(unix)]
    #[test]
    fn run_status_with_timeout_reports_fast_success() {
        let mut cmd = Command::new("true");
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        assert!(run_status_with_timeout(cmd, Duration::from_secs(2)));
    }

    /// `run_capture_stdout_with_timeout` reads stdout on fast success.
    #[cfg(unix)]
    #[test]
    fn run_capture_stdout_with_timeout_returns_stdout() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf gemini-cli\\ 0.26.0"]);
        let stdout = run_capture_stdout_with_timeout(cmd, Duration::from_secs(2))
            .expect("should capture stdout");
        assert_eq!(String::from_utf8_lossy(&stdout), "gemini-cli 0.26.0");
    }

    /// `run_capture_stdout_with_timeout` kills a child that exceeds the
    /// deadline and returns `None`.
    #[cfg(unix)]
    #[test]
    fn run_capture_stdout_with_timeout_kills_hung_child() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 10"]);
        let start = Instant::now();
        let out = run_capture_stdout_with_timeout(cmd, Duration::from_millis(300));
        let elapsed = start.elapsed();
        assert!(out.is_none(), "hung child should not produce stdout");
        assert!(
            elapsed < Duration::from_secs(2),
            "timeout not enforced: took {elapsed:?}"
        );
    }
}
