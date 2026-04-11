//! amux agent wrapper — cross-platform Rust reimplementation of the bash
//! wrappers in `resources/bin/{claude,gemini,codex}`. Built as a single
//! binary that self-dispatches on argv[0] so a single copy installed as
//! `claude.exe`, `gemini.exe`, etc. wraps the matching agent.
//!
//! The wrapper is the **Windows-native replacement** for the POSIX shell
//! wrappers — it avoids the `.cmd`/`.ps1` chaining, PATHEXT, and
//! PowerShell execution-policy pitfalls that would come with any shell
//! approach on Windows. Unix continues to use the bash wrappers today;
//! a future cleanup can swap those out too.
//!
//! Installation: `amux-core::shell::install_agent_wrappers_at` on Windows
//! locates this binary next to `amux.exe` (via `std::env::current_exe`)
//! and copies it into `~/.config/amux/bin/claude.exe` (and friends) each
//! time amux starts a new pane.
//!
//! ## How a wrapped invocation runs
//!
//! 1. User (or amux pane shell) runs `claude.exe`.
//! 2. Windows resolves `claude.exe` via PATH and launches our binary.
//! 3. `main()` inspects `argv[0]` to decide which agent to wrap.
//! 4. The per-agent handler locates the **real** agent binary in PATH,
//!    skipping our own directory so we don't recurse.
//! 5. If we're not inside an amux pane (`AMUX_SURFACE_ID` unset, socket
//!    down, etc.), we passthrough: spawn the real agent with the
//!    unmodified argv and propagate its exit code.
//! 6. Otherwise, we construct the agent-specific hook injection (Claude:
//!    `--settings` JSON; Gemini: temp settings file pointed at by
//!    `GEMINI_CLI_SYSTEM_SETTINGS_PATH`), spawn the real agent, and
//!    propagate its exit code.
//!
//! Note on exec semantics: on Unix, the bash wrappers use `exec` to
//! replace the process image. This Rust wrapper uses `Command::status`
//! which spawns a child and waits — less efficient but portable. On
//! Windows there's no process-replacement primitive, so waiting is the
//! only option anyway.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

mod agents;
mod amux;

/// Exit code returned when we can't locate the real agent binary.
/// Matches the sh idiom of `127` for "command not found".
const EXIT_NOT_FOUND: u8 = 127;

fn main() -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().collect();
    let argv0 = args.first().cloned().unwrap_or_default();
    let agent_name = match detect_agent(&argv0) {
        Some(name) => name,
        None => {
            eprintln!(
                "amux-agent-wrapper: could not determine agent from argv[0] {:?}. \
                 Invoke this binary via a copy named claude.exe, gemini.exe, etc.",
                argv0
            );
            return ExitCode::from(1);
        }
    };

    // Pass argv[1..] to the real agent; argv[0] is ours.
    let forward_args: Vec<OsString> = args.iter().skip(1).cloned().collect();

    let result = match agent_name {
        Agent::Claude => agents::claude::run(&forward_args),
        Agent::Gemini => agents::gemini::run(&forward_args),
    };

    match result {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(1)
        }
    }
}

/// Agent kinds this wrapper knows how to wrap. Codex is intentionally not
/// implemented on Windows yet — its integration requires a symlinked
/// `CODEX_HOME` which needs elevated privileges on Windows without
/// Developer Mode. Tracked as a follow-up to the Windows gap plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Agent {
    Claude,
    Gemini,
}

/// Detect which agent we should wrap based on `argv[0]`. Strips any
/// directory components and extension, then matches a known agent stem.
/// Returns `None` if the stem doesn't match — the binary refuses to run
/// rather than guess, so a misnamed install surfaces loudly.
pub(crate) fn detect_agent(argv0: &std::ffi::OsStr) -> Option<Agent> {
    let stem = Path::new(argv0).file_stem()?.to_str()?.to_ascii_lowercase();
    match stem.as_str() {
        "claude" => Some(Agent::Claude),
        "gemini" => Some(Agent::Gemini),
        _ => None,
    }
}

/// Walk `PATH` for `name`, skipping `skip_dir` (used to avoid finding
/// our own wrapper when looking for the real agent binary). Honours
/// `PATHEXT` on Windows so `find_real_agent("claude", ...)` matches
/// `claude.exe` / `claude.cmd`. Returns the absolute path of the first
/// executable match, or `None`.
pub(crate) fn find_real_agent(name: &str, skip_dir: &Path) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;

    #[cfg(windows)]
    let extensions: Vec<String> = {
        let raw = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_string());
        std::iter::once(String::new())
            .chain(
                raw.split(';')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
            )
            .collect()
    };
    #[cfg(unix)]
    let extensions: Vec<String> = vec![String::new()];

    let canonical_skip = std::fs::canonicalize(skip_dir).ok();

    for dir in std::env::split_paths(&path_var) {
        if dir.as_os_str().is_empty() || !dir.is_absolute() {
            continue;
        }
        // Skip our own install dir so we don't pick up the wrapper again.
        if let Some(ref canonical) = canonical_skip {
            if std::fs::canonicalize(&dir).is_ok_and(|d| d == *canonical) {
                continue;
            }
        }
        for ext in &extensions {
            let candidate = if ext.is_empty() {
                dir.join(name)
            } else {
                dir.join(format!("{name}{ext}"))
            };
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Run the real agent with the given argv and propagate its exit code.
/// Returns the u8 exit code (127 if we can't even spawn). Stdio is
/// inherited so the agent's TUI works normally.
pub(crate) fn spawn_and_wait(real: &Path, args: &[OsString]) -> u8 {
    let mut cmd = Command::new(real);
    cmd.args(args);
    // Inherit stdin/stdout/stderr — the agent is interactive.
    match cmd.status() {
        Ok(status) => status
            .code()
            .and_then(|c| u8::try_from(c & 0xff).ok())
            .unwrap_or(1),
        Err(e) => {
            eprintln!(
                "amux-agent-wrapper: failed to spawn {}: {e}",
                real.display()
            );
            EXIT_NOT_FOUND
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_agent_from_basename() {
        assert_eq!(detect_agent("claude".as_ref()), Some(Agent::Claude));
        assert_eq!(detect_agent("gemini".as_ref()), Some(Agent::Gemini));
        assert_eq!(detect_agent("claude.exe".as_ref()), Some(Agent::Claude));
        assert_eq!(detect_agent("gemini.exe".as_ref()), Some(Agent::Gemini));
    }

    #[test]
    fn detect_agent_from_absolute_path() {
        // Unix-style absolute path.
        assert_eq!(
            detect_agent("/Users/me/.config/amux/bin/claude".as_ref()),
            Some(Agent::Claude)
        );
        // Windows-style path — on unix std::path treats `\` as a literal, so
        // use forward slashes here to stay portable.
        assert_eq!(
            detect_agent("C:/Users/me/AppData/Roaming/amux/bin/gemini.exe".as_ref()),
            Some(Agent::Gemini)
        );
    }

    #[test]
    fn detect_agent_is_case_insensitive() {
        assert_eq!(detect_agent("Claude".as_ref()), Some(Agent::Claude));
        assert_eq!(detect_agent("CLAUDE.EXE".as_ref()), Some(Agent::Claude));
        assert_eq!(detect_agent("GeMiNi".as_ref()), Some(Agent::Gemini));
    }

    #[test]
    fn detect_agent_rejects_unknown_names() {
        assert_eq!(detect_agent("codex".as_ref()), None);
        assert_eq!(detect_agent("amux-agent-wrapper".as_ref()), None);
        assert_eq!(detect_agent("bash".as_ref()), None);
        assert_eq!(detect_agent("".as_ref()), None);
    }
}
