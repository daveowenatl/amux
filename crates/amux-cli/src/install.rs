//! Agent hook installation and shell integration setup.
//!
//! Installs/uninstalls Claude Code hooks into the user's settings file
//! and installs shell integration scripts (bash/zsh precmd hooks that
//! report CWD changes to amux).

pub fn install_claude_hooks() -> anyhow::Result<()> {
    // Hooks are now injected automatically via a claude wrapper script that
    // amux-app writes to ~/.config/amux/bin/claude and prepends to PATH.
    // This command cleans up any old settings.json hooks and informs the user.
    let removed = cleanup_legacy_claude_hooks()?;
    if removed {
        println!("Cleaned up legacy hooks from ~/.claude/settings.json.");
    }
    println!("Claude Code hooks are now automatic — no manual installation needed.");
    println!("amux injects hooks via a wrapper script when Claude Code is launched inside amux.");
    println!("Hooks only activate inside amux terminals; outside amux, Claude Code runs normally.");
    Ok(())
}

/// Remove any amux claude-hook entries from ~/.claude/settings.json.
/// Returns true if any were removed.
pub fn cleanup_legacy_claude_hooks() -> anyhow::Result<bool> {
    let settings_path = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?
        .join(".claude")
        .join("settings.json");

    if !settings_path.exists() {
        return Ok(false);
    }

    let content = std::fs::read_to_string(&settings_path)?;
    let mut settings: serde_json::Value = serde_json::from_str(&content)?;

    let mut removed_any = false;
    if let Some(hooks) = settings.get_mut("hooks") {
        if let Some(hooks_obj) = hooks.as_object_mut() {
            for (_event, entries) in hooks_obj.iter_mut() {
                if let Some(arr) = entries.as_array_mut() {
                    let before = arr.len();
                    arr.retain(|entry| {
                        !entry
                            .get("hooks")
                            .and_then(|h| h.as_array())
                            .map(|hooks| {
                                hooks.iter().any(|h| {
                                    h.get("command")
                                        .and_then(|c| c.as_str())
                                        .is_some_and(|c| c.contains("claude-hook"))
                                })
                            })
                            .unwrap_or(false)
                    });
                    if arr.len() < before {
                        removed_any = true;
                    }
                }
            }
            // Remove empty event arrays
            hooks_obj.retain(|_, v| v.as_array().map(|a| !a.is_empty()).unwrap_or(true));
            if hooks_obj.is_empty() {
                settings.as_object_mut().unwrap().remove("hooks");
            }
        }
    }

    if removed_any {
        let formatted = serde_json::to_string_pretty(&settings)?;
        std::fs::write(&settings_path, formatted)?;
    }

    Ok(removed_any)
}

pub fn uninstall_claude_hooks() -> anyhow::Result<()> {
    let removed = cleanup_legacy_claude_hooks()?;
    if removed {
        println!("Claude Code hooks removed from ~/.claude/settings.json.");
    } else {
        println!("No amux hooks found in ~/.claude/settings.json.");
    }
    Ok(())
}

pub fn install_shell_integration() -> anyhow::Result<()> {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("amux")
        .join("shell");
    std::fs::create_dir_all(&config_dir)?;

    let zsh_script = include_str!("../../../resources/shell-integration/amux-zsh-integration.zsh");
    let bash_script =
        include_str!("../../../resources/shell-integration/amux-bash-integration.bash");

    let zsh_path = config_dir.join("amux-zsh-integration.zsh");
    let bash_path = config_dir.join("amux-bash-integration.bash");

    std::fs::write(&zsh_path, zsh_script)?;
    std::fs::write(&bash_path, bash_script)?;

    println!("Shell integration scripts installed to:");
    println!("  {}", zsh_path.display());
    println!("  {}", bash_path.display());
    println!();
    println!("Add one of the following to your shell config:");
    println!();
    println!("  # For zsh (~/.zshrc):");
    println!(
        "  [[ -n \"$AMUX_SOCKET_PATH\" ]] && source \"{}\"",
        zsh_path.display()
    );
    println!();
    println!("  # For bash (~/.bashrc):");
    println!(
        "  [[ -n \"$AMUX_SOCKET_PATH\" ]] && source \"{}\"",
        bash_path.display()
    );

    Ok(())
}
