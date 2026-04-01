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
        if let AdvanceResult::Eof = pane.advance() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    println!("--- {name} ---");
    println!("  Dimensions: {:?}", pane.dimensions());
    println!("  Cursor: {:?}", pane.cursor());
    println!("  Alive: {}", pane.is_alive());

    // Plain text
    let text = pane.read_screen_text();
    println!("  Screen text ({} chars):", text.len());
    for line in text.lines() {
        println!("    {line:?}");
    }

    // Cell-level access (what the GPU renderer uses)
    let rows = pane.read_screen_cells(0);
    let non_empty: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.cells.iter().any(|c| !c.text.is_empty() && c.text != " "))
        .collect();
    println!(
        "  Screen cells: {} rows ({} non-empty)",
        rows.len(),
        non_empty.len()
    );
    for (i, row) in &non_empty {
        let line_text: String = row.cells.iter().map(|c| c.text.as_str()).collect();
        let has_bold = row.cells.iter().any(|c| c.bold);
        let has_color = row
            .cells
            .iter()
            .any(|c| c.fg.0 != 1.0 || c.fg.1 != 1.0 || c.fg.2 != 1.0);
        println!(
            "    row {i}: {:?} bold={has_bold} color={has_color} wrapped={}",
            line_text.trim_end(),
            row.wrapped,
        );
    }

    // Line-spec reading
    let last2 = pane.read_screen_lines("-2", false);
    println!("  read_screen_lines(\"-2\"):");
    for line in last2.lines() {
        println!("    {line:?}");
    }

    // Search
    let hits = pane.search_scrollback("plain");
    println!("  search_scrollback(\"plain\"): {hits:?}");

    // Scrollback text
    let scrollback = pane.read_scrollback_text(100);
    println!(
        "  read_scrollback_text(100): {} chars, {} lines",
        scrollback.len(),
        scrollback.lines().count()
    );

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
    // Use ANSI color codes to test attribute extraction
    let script = concat!(
        "printf '\\033[1mBOLD\\033[0m \\033[3mITALIC\\033[0m \\033[31mRED\\033[0m\\n'; ",
        "printf 'plain text\\n'; ",
        "printf '\\033[4munderlined\\033[0m\\n'; ",
        "exit 0"
    );

    println!("=== Backend comparison (with attributes) ===");
    println!("Command: bash -c <script with ANSI colors>\n");

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
