//! Smoke test for the GhosttyPane backend.
//!
//! Spawns a shell, runs commands, and exercises the TerminalBackend trait.
//!
//! Run with:
//!   cargo run -p amux-term --example ghostty_smoke

use std::thread;
use std::time::Duration;

use amux_term::backend::TerminalBackend;
use amux_term::ghostty_pane::GhosttyPane;
use portable_pty::CommandBuilder;

fn main() {
    println!("=== GhosttyPane smoke test ===\n");

    let mut cmd = CommandBuilder::new("bash");
    cmd.args([
        "--norc",
        "--noprofile",
        "-c",
        "echo 'hello from ghostty'; echo 'line two'; sleep 0.2; exit 42",
    ]);

    let mut pane = GhosttyPane::spawn(80, 24, cmd).expect("spawn failed");
    println!("Spawned. PID: {:?}", pane.child_pid());
    println!("Dimensions: {:?}", pane.dimensions());

    // Let the shell run
    for _ in 0..20 {
        match pane.advance() {
            amux_term::AdvanceResult::Read(_) => {}
            amux_term::AdvanceResult::WouldBlock => {}
            amux_term::AdvanceResult::Eof => break,
        }
        thread::sleep(Duration::from_millis(50));
    }

    println!("\n--- Screen text ---");
    let text = pane.read_screen_text();
    if text.is_empty() {
        println!("(empty)");
    } else {
        for (i, line) in text.lines().enumerate() {
            println!("  {i}: {line:?}");
        }
    }

    println!("\n--- State ---");
    println!("Title: {:?}", pane.title());
    println!("Cursor: {:?}", pane.cursor());
    println!("Alt screen: {}", pane.is_alt_screen_active());
    println!("Bracketed paste: {}", pane.bracketed_paste_enabled());
    println!("Scrollback rows: {}", pane.scrollback_rows());
    println!("Alive: {}", pane.is_alive());

    let palette = pane.palette();
    println!(
        "Palette: fg=({:.2},{:.2},{:.2}) bg=({:.2},{:.2},{:.2}) {} colors",
        palette.foreground.0,
        palette.foreground.1,
        palette.foreground.2,
        palette.background.0,
        palette.background.1,
        palette.background.2,
        palette.colors.len()
    );

    if let Some(exit) = pane.exit_status() {
        println!("Exit: code={} success={}", exit.exit_code(), exit.success());
    }

    println!("\n=== Done ===");
}
