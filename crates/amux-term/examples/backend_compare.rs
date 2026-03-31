//! Side-by-side comparison: TerminalPane (wezterm) vs GhosttyPane (libghostty).
//!
//! Runs the same command through both backends and compares what the
//! TerminalBackend trait reports. Validates the abstraction is sound.
//!
//! Run with:
//!   cargo run -p amux-term --features libghostty --example backend_compare

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use amux_term::backend::TerminalBackend;
use amux_term::config::AmuxTermConfig;
use amux_term::ghostty_pane::GhosttyPane;
use amux_term::pane::{AdvanceResult, TerminalPane};
use portable_pty::CommandBuilder;

fn run_backend(name: &str, pane: &mut dyn TerminalBackend) {
    // Let the shell run
    for _ in 0..30 {
        match pane.advance() {
            AdvanceResult::Eof => break,
            _ => {}
        }
        thread::sleep(Duration::from_millis(50));
    }

    println!("--- {name} ---");
    println!("  Dimensions: {:?}", pane.dimensions());
    println!("  Cursor: {:?}", pane.cursor());
    println!("  Alt screen: {}", pane.is_alt_screen_active());
    println!("  Bracketed paste: {}", pane.bracketed_paste_enabled());
    println!("  Scrollback rows: {}", pane.scrollback_rows());
    println!("  Alive: {}", pane.is_alive());

    let text = pane.read_screen_text();
    println!("  Screen text ({} chars):", text.len());
    for line in text.lines() {
        println!("    {line:?}");
    }

    if let Some(exit) = pane.exit_status() {
        println!(
            "  Exit: code={} success={}",
            exit.exit_code(),
            exit.success()
        );
    }
    println!();
}

fn main() {
    let script = "echo 'ALPHA'; echo 'BRAVO'; echo 'CHARLIE'; exit 0";

    println!("=== Backend comparison ===");
    println!("Command: bash -c {script:?}\n");

    // --- wezterm-term backend ---
    let mut cmd1 = CommandBuilder::new("bash");
    cmd1.args(["--norc", "--noprofile", "-c", script]);
    let config = Arc::new(AmuxTermConfig::default());
    let mut wezterm_pane = TerminalPane::spawn(80, 24, cmd1, config).expect("wezterm spawn failed");
    run_backend("wezterm-term (TerminalPane)", &mut wezterm_pane);

    // --- libghostty-vt backend ---
    let mut cmd2 = CommandBuilder::new("bash");
    cmd2.args(["--norc", "--noprofile", "-c", script]);
    let mut ghostty_pane = GhosttyPane::spawn(80, 24, cmd2).expect("ghostty spawn failed");
    run_backend("libghostty-vt (GhosttyPane)", &mut ghostty_pane);

    println!("=== Both backends exercised through TerminalBackend trait ===");
}
