//! Shell detection and integration setup.
//!
//! These functions prepare the environment for shell integration scripts
//! and the claude wrapper binary without depending on any terminal crate.

use portable_pty::CommandBuilder;

/// Return the user's default shell.
pub fn default_shell() -> String {
    #[cfg(unix)]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
    }
    #[cfg(windows)]
    {
        std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
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
        // PowerShell/pwsh support for Gemini is issue #166; for now only the
        // claude.cmd shim is installed on Windows.
        let wrapper_path = bin_dir.join("claude.cmd");
        let wrapper_content = "@echo off\r\nclaude.exe %*\r\n";

        let needs_write = std::fs::read_to_string(&wrapper_path)
            .map(|existing| existing != wrapper_content)
            .unwrap_or(true);

        if needs_write && std::fs::write(&wrapper_path, wrapper_content).is_err() {
            return None;
        }
    }

    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
