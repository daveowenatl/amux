//! Manual shell integration setup.
//!
//! Installs shell integration scripts to `~/.config/amux/shell/` and prints
//! instructions for sourcing them. This is the opt-in path for users with
//! custom shell setups who don't want amux's automatic ZDOTDIR/PROMPT_COMMAND
//! injection. Users launching shells inside amux panes don't need this — the
//! same scripts are written automatically by `ensure_shell_integration_dir`
//! in `amux_core::shell` and sourced via environment overrides.
//!
//! Historical note: this module previously held `install_claude_hooks`,
//! `uninstall_claude_hooks`, and `cleanup_legacy_claude_hooks`. Those were
//! removed when the `install-hooks` CLI became a no-op — Claude Code hooks
//! now inject at runtime via the `~/.config/amux/bin/claude` wrapper script
//! (see `crates/amux-core/src/shell.rs::ensure_agent_wrapper_dir`). The
//! one-time cleanup of legacy `~/.claude/settings.json` entries left by old
//! amux versions now happens automatically at app startup.

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
