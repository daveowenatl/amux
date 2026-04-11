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

/// Write shell integration files to ~/.config/amux/shell/ and set env vars to
/// auto-inject them. For zsh: ZDOTDIR override. For bash: PROMPT_COMMAND bootstrap.
/// Matching cmux's approach — no user dotfile modification required.
pub fn inject_shell_integration(shell: &str, cmd: &mut CommandBuilder) {
    let shell_name = std::path::Path::new(shell)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or("");

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
    if std::fs::create_dir_all(&bin_dir).is_err() {
        return None;
    }

    #[cfg(unix)]
    {
        let wrappers: &[(&str, &str)] = &[
            ("claude", include_str!("../../../resources/bin/claude")),
            ("gemini", include_str!("../../../resources/bin/gemini")),
        ];
        for (name, content) in wrappers {
            let path = bin_dir.join(name);
            let needs_write = std::fs::read_to_string(&path)
                .map(|existing| existing != *content)
                .unwrap_or(true);
            if needs_write {
                if std::fs::write(&path, content).is_err() {
                    return None;
                }
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
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

    Some(bin_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_agent_wrapper_dir_writes_both_wrappers() {
        let bin_dir = ensure_agent_wrapper_dir().expect("should return dir");
        #[cfg(unix)]
        {
            assert!(bin_dir.join("claude").exists(), "claude wrapper missing");
            assert!(bin_dir.join("gemini").exists(), "gemini wrapper missing");
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(bin_dir.join("gemini"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "gemini wrapper not executable");
        }
    }
}
