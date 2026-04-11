//! Shell detection and integration setup.
//!
//! These functions prepare the environment for shell integration scripts
//! and the claude wrapper binary without depending on any terminal crate.

use portable_pty::CommandBuilder;

/// Return the default shell amux should spawn in new panes.
///
/// Unix: `$SHELL`, falling back to `/bin/bash`.
///
/// Windows: prefers `pwsh.exe` (PowerShell 7+) if it's on `PATH` so the
/// user automatically gets the amux shell integration that ships in
/// `resources/shell-integration/amux-pwsh-integration.ps1`. Falls back
/// to `$COMSPEC` (typically `cmd.exe`) when pwsh isn't installed. Users
/// who want a specific shell can override this in `config.toml` via the
/// top-level `shell` key — see [`resolve_shell`].
pub fn default_shell() -> String {
    #[cfg(unix)]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
    }
    #[cfg(windows)]
    {
        if let Some(pwsh) = find_on_path("pwsh.exe") {
            return pwsh;
        }
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
    }
}

/// Resolve the shell amux should spawn for a new pane. Config override
/// wins, falling through to [`default_shell`]. An absolute-path override
/// is returned verbatim; a bare name (`"pwsh"`, `"bash"`) is resolved
/// against `PATH` — if resolution fails, the bare name is still returned
/// so portable-pty can surface a clearer spawn error than "not found".
pub fn resolve_shell(config_override: Option<&str>) -> String {
    if let Some(override_value) = config_override {
        let trimmed = override_value.trim();
        if !trimmed.is_empty() {
            let path = std::path::Path::new(trimmed);
            if path.is_absolute() {
                return trimmed.to_string();
            }
            if let Some(resolved) = find_on_path(trimmed) {
                return resolved;
            }
            return trimmed.to_string();
        }
    }
    default_shell()
}

/// Walk `PATH` looking for an executable with the given file name. On
/// Windows, honours `PATHEXT` so `find_on_path("pwsh")` matches `pwsh.exe`.
/// On Unix, verifies the execute bit is set so we never return a candidate
/// that would fail to spawn. Relative `PATH` entries are skipped to avoid
/// PATH-hijacking risk (e.g. a `.` entry pointing the shell resolver at
/// whatever binary happens to be in the current working directory).
/// Returns the first match as an absolute path string, or `None`.
fn find_on_path(name: &str) -> Option<String> {
    let path_var = std::env::var_os("PATH")?;
    #[cfg(windows)]
    let extensions: Vec<String> = {
        let raw = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_string());
        // An empty entry represents the original `name` without any extension
        // appended — matches the behaviour of `where.exe` and `Get-Command`.
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

    for dir in std::env::split_paths(&path_var) {
        // Skip empty and relative entries. Absolute-only guarantees the
        // returned path is safe to embed in command spawns and matches
        // the docstring contract above.
        if dir.as_os_str().is_empty() || !dir.is_absolute() {
            continue;
        }
        for ext in &extensions {
            let candidate = if ext.is_empty() {
                dir.join(name)
            } else {
                dir.join(format!("{name}{ext}"))
            };
            if !candidate.is_file() {
                continue;
            }
            if !is_executable(&candidate) {
                continue;
            }
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

/// Returns true when the path at `candidate` is executable by the current
/// process. On Unix, checks the file-mode execute bits. On Windows, the
/// `.exe`/`.cmd`/`.bat` extension probe done by `find_on_path` already
/// filters out non-executable files so we accept any regular file.
fn is_executable(candidate: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Ok(meta) = std::fs::metadata(candidate) else {
            return false;
        };
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(windows)]
    {
        let _ = candidate;
        true
    }
}

/// Normalize a shell file name to its stem. On Windows the shell binary is
/// typically reported as `pwsh.exe`, `bash.exe`, `cmd.exe`; on Unix it's
/// just `zsh`, `bash`, etc. Using the stem lets the match arm below work
/// identically on both platforms.
fn shell_stem(shell: &str) -> &str {
    std::path::Path::new(shell)
        .file_stem()
        .and_then(|f| f.to_str())
        .unwrap_or("")
}

/// Returns true if the given shell binary should get scrollback restore and
/// other integration-driven features (git reporting, OSC 133 marks, etc.).
/// Callers gate their integration-specific plumbing on this so a user with
/// cmd.exe or an unrecognized shell still gets a working terminal.
pub fn has_shell_integration(shell: &str) -> bool {
    matches!(shell_stem(shell), "zsh" | "bash" | "pwsh")
}

/// Write shell integration files to ~/.config/amux/shell/ and set env vars to
/// auto-inject them. For zsh: ZDOTDIR override. For bash: PROMPT_COMMAND
/// bootstrap. For pwsh: a `-NoLogo -NoExit -Command` bootstrap that loads the
/// user's $PROFILE first then dot-sources our integration script.
/// Matching cmux's approach — no user dotfile modification required.
pub fn inject_shell_integration(shell: &str, cmd: &mut CommandBuilder) {
    let shell_name = shell_stem(shell);

    let Some(integration_dir) = ensure_shell_integration_dir() else {
        return;
    };

    cmd.env(
        "AMUX_SHELL_INTEGRATION_DIR",
        integration_dir.to_string_lossy().as_ref(),
    );

    match shell_name {
        "zsh" => {
            let zsh_dir = integration_dir.join("zsh");
            // Save user's original ZDOTDIR (if set) so bootstrap can restore it
            if let Ok(original) = std::env::var("ZDOTDIR") {
                cmd.env("AMUX_ZSH_ZDOTDIR", &original);
            }
            cmd.env("ZDOTDIR", zsh_dir.to_string_lossy().as_ref());
        }
        "bash" => {
            // Bootstrap integration via PROMPT_COMMAND on first interactive prompt.
            // Preserve any existing PROMPT_COMMAND so user hooks still fire.
            let bash_script = integration_dir.join("amux-bash-integration.bash");
            let orig = std::env::var("PROMPT_COMMAND").unwrap_or_default();
            let bootstrap = if orig.is_empty() {
                format!(
                    "unset PROMPT_COMMAND; if [[ -r \"{}\" ]]; then source \"{}\"; fi",
                    bash_script.display(),
                    bash_script.display(),
                )
            } else {
                format!(
                    concat!(
                        "unset PROMPT_COMMAND; ",
                        "if [[ -r \"{}\" ]]; then source \"{}\"; fi; ",
                        "{}",
                    ),
                    bash_script.display(),
                    bash_script.display(),
                    orig,
                )
            };
            cmd.env("PROMPT_COMMAND", &bootstrap);
        }
        "pwsh" => {
            // PowerShell 7+ has no ZDOTDIR equivalent. The only reliable
            // zero-user-config injection point is `-Command`. The bootstrap
            // loads the user's $PROFILE first (so any prompt override they
            // define is in place), then dot-sources our integration so we
            // can wrap their prompt.
            //
            // Escape apostrophes in the script path for the single-quoted
            // PowerShell string literal below. PowerShell escapes `'` inside
            // a single-quoted string as `''`.
            let pwsh_script = integration_dir.join("amux-pwsh-integration.ps1");
            let escaped_path = pwsh_script.to_string_lossy().replace('\'', "''");
            let bootstrap = format!("if (Test-Path $PROFILE) {{ . $PROFILE }}; . '{escaped_path}'");
            cmd.arg("-NoLogo");
            cmd.arg("-NoExit");
            cmd.arg("-Command");
            cmd.arg(bootstrap);
        }
        _ => {}
    }
}

/// Ensure shell integration scripts are written to ~/.config/amux/shell/.
/// Returns the directory path, or None on failure.
fn ensure_shell_integration_dir() -> Option<std::path::PathBuf> {
    let config_dir = dirs::config_dir()?.join("amux").join("shell");

    // Write zsh bootstrap files
    let zsh_dir = config_dir.join("zsh");
    if std::fs::create_dir_all(&zsh_dir).is_err() {
        return None;
    }

    // Embed integration scripts at compile time
    let files: &[(&str, &str)] = &[
        (
            "zsh/.zshenv",
            include_str!("../../../resources/shell-integration/zsh/.zshenv"),
        ),
        (
            "zsh/.zprofile",
            include_str!("../../../resources/shell-integration/zsh/.zprofile"),
        ),
        (
            "zsh/.zshrc",
            include_str!("../../../resources/shell-integration/zsh/.zshrc"),
        ),
        (
            "zsh/.zlogin",
            include_str!("../../../resources/shell-integration/zsh/.zlogin"),
        ),
        (
            "amux-zsh-integration.zsh",
            include_str!("../../../resources/shell-integration/amux-zsh-integration.zsh"),
        ),
        (
            "amux-bash-integration.bash",
            include_str!("../../../resources/shell-integration/amux-bash-integration.bash"),
        ),
        (
            "amux-pwsh-integration.ps1",
            include_str!("../../../resources/shell-integration/amux-pwsh-integration.ps1"),
        ),
    ];

    for (name, content) in files {
        let path = config_dir.join(name);
        // Only write if content changed (avoid unnecessary disk writes)
        let needs_write = std::fs::read_to_string(&path)
            .map(|existing| existing != *content)
            .unwrap_or(true);
        if needs_write && std::fs::write(&path, content).is_err() {
            return None;
        }
    }

    Some(config_dir)
}

/// Ensure agent wrapper scripts are written to ~/.config/amux/bin/.
/// Writes both the `claude` and `gemini` wrappers on Unix. Returns the
/// bin directory path, or None on failure.
pub fn ensure_agent_wrapper_dir() -> Option<std::path::PathBuf> {
    // Use ~/.config/amux/bin/ instead of dirs::config_dir() because on macOS
    // that returns ~/Library/Application Support/ which has a space — spaces
    // in PATH entries break many tools.
    let bin_dir = dirs::home_dir()?.join(".config").join("amux").join("bin");
    install_agent_wrappers_at(&bin_dir)?;
    Some(bin_dir)
}

/// Write agent wrapper scripts into `bin_dir`. Extracted so tests can
/// target a tempdir instead of mutating the caller's real ~/.config tree.
pub fn install_agent_wrappers_at(bin_dir: &std::path::Path) -> Option<()> {
    if std::fs::create_dir_all(bin_dir).is_err() {
        return None;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let wrappers: &[(&str, &str)] = &[
            ("claude", include_str!("../../../resources/bin/claude")),
            ("gemini", include_str!("../../../resources/bin/gemini")),
            ("codex", include_str!("../../../resources/bin/codex")),
        ];
        for (name, content) in wrappers {
            let path = bin_dir.join(name);
            let needs_write = std::fs::read_to_string(&path)
                .map(|existing| existing != *content)
                .unwrap_or(true);
            if needs_write && std::fs::write(&path, content).is_err() {
                return None;
            }
            // Always enforce the execute bit. Running this unconditionally
            // (not just on content change) self-heals wrappers whose mode
            // got stripped externally — e.g., cp/rsync from a non-unix fs.
            if std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).is_err() {
                return None;
            }
        }
    }
    #[cfg(windows)]
    {
        // On Windows we install copies of `amux-agent-wrapper.exe` (built as
        // a workspace member — see `crates/amux-agent-wrapper`) renamed to
        // `claude.exe` and `gemini.exe`. The wrapper self-dispatches on its
        // argv[0] filename so one source binary handles all agents. This
        // replaces the legacy `claude.cmd` passthrough shim that gave zero
        // hook integration on Windows.
        //
        // Codex on Windows is still deferred — its integration needs a
        // symlink farm that requires Developer Mode on Windows. Tracked as
        // a follow-up to the Windows gap plan.
        let Some(wrapper_src) = locate_agent_wrapper_binary() else {
            tracing::warn!(
                "Windows agent wrappers skipped: amux-agent-wrapper.exe not found \
                 next to the running amux binary. Claude/Gemini hook injection \
                 will not work in new panes until the wrapper is available."
            );
            return Some(());
        };

        let targets: &[&str] = &["claude.exe", "gemini.exe"];
        for name in targets {
            let dest = bin_dir.join(name);
            if needs_wrapper_copy(&wrapper_src, &dest) {
                // Remove first — `std::fs::copy` on Windows fails if the
                // destination is currently executing (a running claude.exe
                // from a prior pane). Falling through on removal failure
                // lets the copy surface the real error.
                let _ = std::fs::remove_file(&dest);
                if std::fs::copy(&wrapper_src, &dest).is_err() {
                    return None;
                }
            }
        }
    }

    Some(())
}

/// Locate `amux-agent-wrapper.exe` on Windows. Expected to live next to
/// the running `amux.exe` / `amux-app.exe` — the release distribution
/// ships all three binaries in the same directory, and `cargo build
/// --workspace` during development puts them in the same `target/<profile>`.
#[cfg(windows)]
fn locate_agent_wrapper_binary() -> Option<std::path::PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate = dir.join("amux-agent-wrapper.exe");
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

/// Returns true when `dest` is missing or has different content than
/// `src`. Avoids redundant copies on every pane launch. Compared via
/// file length first (cheap) then byte content as a fallback. The
/// wrapper binary is small (a few hundred KB) so a full-file read is
/// acceptable on mismatch.
#[cfg(windows)]
fn needs_wrapper_copy(src: &std::path::Path, dest: &std::path::Path) -> bool {
    let Ok(src_meta) = std::fs::metadata(src) else {
        return false; // source missing — refuse to copy anyway
    };
    let Ok(dest_meta) = std::fs::metadata(dest) else {
        return true;
    };
    if src_meta.len() != dest_meta.len() {
        return true;
    }
    let Ok(src_bytes) = std::fs::read(src) else {
        return false;
    };
    let Ok(dest_bytes) = std::fs::read(dest) else {
        return true;
    };
    src_bytes != dest_bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_shell_honours_absolute_override() {
        // Absolute paths are returned verbatim, even if they don't exist —
        // portable-pty surfaces the spawn error with context. We don't
        // stat-check here because the override is an explicit user choice.
        let path = if cfg!(windows) {
            "C:\\Program Files\\Fish\\bin\\fish.exe"
        } else {
            "/opt/homebrew/bin/fish"
        };
        assert_eq!(resolve_shell(Some(path)), path);
    }

    #[test]
    fn resolve_shell_ignores_empty_or_whitespace_override() {
        // `shell = ""` or `shell = "   "` in config.toml falls back to
        // default_shell() — we don't want to spawn an empty command.
        let fallback = default_shell();
        assert_eq!(resolve_shell(Some("")), fallback);
        assert_eq!(resolve_shell(Some("   ")), fallback);
    }

    #[test]
    fn resolve_shell_none_falls_back_to_default() {
        assert_eq!(resolve_shell(None), default_shell());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_shell_bare_name_resolves_against_path() {
        // `sh` is guaranteed to exist on every Unix runner, so this test
        // doubles as a sanity check for `find_on_path` on Unix.
        let resolved = resolve_shell(Some("sh"));
        assert!(
            resolved.ends_with("/sh") || resolved == "sh",
            "expected resolved sh path or bare fallback, got {resolved}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn resolve_shell_bare_name_returns_input_when_missing() {
        // A made-up name must not crash — we return it verbatim so the
        // subsequent CommandBuilder::new gives a clean "not found" error.
        assert_eq!(
            resolve_shell(Some("definitely-not-a-real-shell-xyz")),
            "definitely-not-a-real-shell-xyz"
        );
    }

    /// Serializes the Windows PATH/PATHEXT tests so parallel test threads
    /// can't stomp each other's env manipulation.
    #[cfg(windows)]
    static WINDOWS_PATH_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard that restores a named env var to its prior value on drop.
    #[cfg(windows)]
    struct EnvGuard {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }
    #[cfg(windows)]
    impl EnvGuard {
        fn set(key: &'static str, value: &std::ffi::OsStr) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: tests are serialized via WINDOWS_PATH_TEST_LOCK.
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }
    }
    #[cfg(windows)]
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: tests are serialized via WINDOWS_PATH_TEST_LOCK.
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    /// Regression for Copilot #3068233897: Windows `find_on_path` must
    /// honour `PATHEXT` so a bare `"pwsh"` override resolves to
    /// `pwsh.exe` inside a PATH directory, and the empty-extension
    /// case matches before extension-appended candidates (matches the
    /// `where.exe` / `Get-Command` lookup order).
    #[cfg(windows)]
    #[test]
    fn windows_find_on_path_pathext_prefers_exe() {
        let _guard = WINDOWS_PATH_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().expect("tempdir");
        let exe = tmp.path().join("pwsh.exe");
        std::fs::write(&exe, b"dummy").expect("write exe");

        let path_env = EnvGuard::set("PATH", tmp.path().as_os_str());
        let pathext_env = EnvGuard::set("PATHEXT", std::ffi::OsStr::new(".EXE;.CMD;.BAT;.COM"));

        let resolved = resolve_shell(Some("pwsh"));
        drop(path_env);
        drop(pathext_env);
        // PATHEXT case is preserved in the constructed candidate, so the
        // resolved file_name may be "pwsh.EXE" (uppercase) even though the
        // on-disk file is "pwsh.exe". Windows filesystems are
        // case-insensitive, so compare ignoring ASCII case.
        let file_name = std::path::PathBuf::from(&resolved)
            .file_name()
            .expect("file_name")
            .to_string_lossy()
            .into_owned();
        assert!(
            file_name.eq_ignore_ascii_case("pwsh.exe"),
            "expected pwsh.exe (any case), got {file_name}"
        );
    }

    /// Windows: when only a `.cmd` shim exists (no `.exe`), PATHEXT
    /// iteration still resolves via the extension fallback.
    #[cfg(windows)]
    #[test]
    fn windows_find_on_path_falls_back_to_cmd_extension() {
        let _guard = WINDOWS_PATH_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().expect("tempdir");
        let cmd_shim = tmp.path().join("mytool.cmd");
        std::fs::write(&cmd_shim, b"@echo off\r\n").expect("write cmd");

        let path_env = EnvGuard::set("PATH", tmp.path().as_os_str());
        let pathext_env = EnvGuard::set("PATHEXT", std::ffi::OsStr::new(".EXE;.CMD;.BAT;.COM"));

        let resolved = resolve_shell(Some("mytool"));
        drop(path_env);
        drop(pathext_env);
        // Same PATHEXT-case caveat as windows_find_on_path_pathext_prefers_exe.
        let file_name = std::path::PathBuf::from(&resolved)
            .file_name()
            .expect("file_name")
            .to_string_lossy()
            .into_owned();
        assert!(
            file_name.eq_ignore_ascii_case("mytool.cmd"),
            "expected mytool.cmd (any case), got {file_name}"
        );
    }

    /// Windows: a relative PATH entry (e.g. `.`) must be skipped so we
    /// don't fall prey to PATH hijacking. The resolver must return the
    /// bare name verbatim when no absolute match exists.
    #[cfg(windows)]
    #[test]
    fn windows_find_on_path_skips_relative_entries() {
        let _guard = WINDOWS_PATH_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().expect("tempdir");
        // Write an "evil" exe in the temp dir and set PATH=. while the
        // CWD is the temp dir. The resolver should NOT return it.
        let evil = tmp.path().join("pwsh.exe");
        std::fs::write(&evil, b"dummy").expect("write evil");
        let _cwd_guard = {
            let prev_cwd = std::env::current_dir().expect("cwd");
            std::env::set_current_dir(tmp.path()).expect("set cwd");
            // Move `prev_cwd` into a closure that restores on drop via a
            // `defer`-style helper. Simpler: a struct guard.
            struct CwdGuard(std::path::PathBuf);
            impl Drop for CwdGuard {
                fn drop(&mut self) {
                    let _ = std::env::set_current_dir(&self.0);
                }
            }
            CwdGuard(prev_cwd)
        };

        let path_env = EnvGuard::set("PATH", std::ffi::OsStr::new("."));
        let pathext_env = EnvGuard::set("PATHEXT", std::ffi::OsStr::new(".EXE"));

        let resolved = resolve_shell(Some("pwsh"));
        drop(path_env);
        drop(pathext_env);

        assert_eq!(
            resolved, "pwsh",
            "relative PATH entry must not resolve — got {resolved:?}"
        );
    }

    #[test]
    fn shell_stem_strips_extensions_and_directories() {
        // Bare names (no path, no extension) — portable across platforms.
        assert_eq!(shell_stem("pwsh"), "pwsh");
        assert_eq!(shell_stem("bash"), "bash");
        assert_eq!(shell_stem(""), "");
        // Extensions strip on both platforms; Path::file_stem is extension-aware
        // regardless of directory separators.
        assert_eq!(shell_stem("bash.exe"), "bash");
        assert_eq!(shell_stem("pwsh.exe"), "pwsh");
    }

    #[cfg(unix)]
    #[test]
    fn shell_stem_on_unix_handles_absolute_paths() {
        assert_eq!(shell_stem("/bin/bash"), "bash");
        assert_eq!(shell_stem("/usr/bin/zsh"), "zsh");
        assert_eq!(shell_stem("/opt/homebrew/bin/pwsh"), "pwsh");
    }

    #[cfg(windows)]
    #[test]
    fn shell_stem_on_windows_handles_backslash_paths() {
        assert_eq!(
            shell_stem("C:\\Program Files\\PowerShell\\7\\pwsh.exe"),
            "pwsh"
        );
        assert_eq!(shell_stem("C:\\Windows\\System32\\cmd.exe"), "cmd");
        // Forward-slash Windows paths are accepted by the std Path APIs on
        // Windows too, so users pointing at `C:/.../pwsh.exe` still resolve.
        assert_eq!(shell_stem("C:/Program Files/PowerShell/7/pwsh.exe"), "pwsh");
    }

    #[test]
    fn has_shell_integration_covers_supported_shells_by_stem() {
        // Bare-name forms (what file_stem on a bare name returns) — these
        // must always match regardless of host platform.
        assert!(has_shell_integration("bash"));
        assert!(has_shell_integration("zsh"));
        assert!(has_shell_integration("pwsh"));
        assert!(has_shell_integration("bash.exe"));
        assert!(has_shell_integration("pwsh.exe"));

        // Explicitly NOT supported: cmd.exe, fish, Windows PowerShell 5.1,
        // sh, ksh. These return a functional terminal pane without scrollback
        // restore or status hooks.
        assert!(!has_shell_integration("cmd.exe"));
        assert!(!has_shell_integration("sh"));
        assert!(!has_shell_integration("fish"));
        assert!(!has_shell_integration("powershell.exe"));
        assert!(!has_shell_integration(""));
    }

    #[cfg(unix)]
    #[test]
    fn has_shell_integration_with_unix_absolute_paths() {
        assert!(has_shell_integration("/bin/bash"));
        assert!(has_shell_integration("/usr/bin/zsh"));
        assert!(has_shell_integration("/opt/homebrew/bin/pwsh"));
        assert!(!has_shell_integration("/bin/sh"));
        assert!(!has_shell_integration("/usr/bin/fish"));
    }

    #[cfg(windows)]
    #[test]
    fn has_shell_integration_with_windows_paths() {
        assert!(has_shell_integration(
            "C:\\Program Files\\PowerShell\\7\\pwsh.exe"
        ));
        assert!(!has_shell_integration("C:\\Windows\\System32\\cmd.exe"));
        assert!(!has_shell_integration(
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
        ));
    }

    #[test]
    fn install_agent_wrappers_at_writes_all_wrappers() {
        let tmp = tempfile::tempdir().expect("tempdir");
        install_agent_wrappers_at(tmp.path()).expect("install");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in ["claude", "gemini", "codex"] {
                assert!(tmp.path().join(name).exists(), "{name} wrapper missing");
                let mode = std::fs::metadata(tmp.path().join(name))
                    .unwrap()
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o111, 0o111, "{name} wrapper not executable");
            }
        }
    }

    /// Regression: if a wrapper already exists with correct contents but the
    /// execute bit got stripped, re-running the installer should restore it.
    /// Covers every wrapper so a bug in the chmod loop can't silently miss one.
    #[cfg(unix)]
    #[test]
    fn install_agent_wrappers_at_reapplies_chmod() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().expect("tempdir");
        install_agent_wrappers_at(tmp.path()).expect("first install");

        for name in ["claude", "gemini", "codex"] {
            let path = tmp.path().join(name);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
                .expect("strip +x");
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o111,
                0,
                "precondition: +x stripped on {name}"
            );
        }

        install_agent_wrappers_at(tmp.path()).expect("second install");
        for name in ["claude", "gemini", "codex"] {
            let mode = std::fs::metadata(tmp.path().join(name))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o111,
                0o111,
                "installer should re-enforce +x on {name}"
            );
        }
    }
}
