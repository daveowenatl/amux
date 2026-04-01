//! Integration tests for GhosttyPane via the TerminalBackend trait.
//!
//! Mirrors pty_integration.rs but exercises the libghostty-vt backend.
//! Run with: cargo test -p amux-term --features libghostty

#![cfg(feature = "libghostty")]

use std::thread;
use std::time::Duration;

use portable_pty::CommandBuilder;

use amux_term::backend::TerminalBackend;
use amux_term::ghostty_pane::GhosttyPane;
use amux_term::pane::AdvanceResult;

/// Helper: spawn a GhosttyPane running a bash script, advance until EOF or timeout.
fn spawn_and_run(script: &str) -> Box<GhosttyPane<'static, 'static>> {
    let mut cmd = CommandBuilder::new("bash");
    cmd.args(["--norc", "--noprofile", "-c", script]);
    let mut pane = Box::new(GhosttyPane::spawn(80, 24, cmd).expect("spawn failed"));

    for _ in 0..60 {
        if let AdvanceResult::Eof = pane.advance() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    pane
}

#[test]
fn dimensions_match_spawn_size() {
    let cmd = CommandBuilder::new("true");
    let pane = GhosttyPane::spawn(120, 40, cmd).expect("spawn failed");
    let (cols, rows) = pane.dimensions();
    assert_eq!(cols, 120);
    assert_eq!(rows, 40);
}

#[test]
fn resize_updates_dimensions() {
    let cmd = CommandBuilder::new("true");
    let mut pane = GhosttyPane::spawn(80, 24, cmd).expect("spawn failed");
    pane.resize(100, 30).expect("resize failed");
    let (cols, rows) = pane.dimensions();
    assert_eq!(cols, 100);
    assert_eq!(rows, 30);
}

#[test]
fn screen_text_contains_output() {
    let mut cmd = CommandBuilder::new("bash");
    cmd.args([
        "--norc",
        "--noprofile",
        "-c",
        "printf 'hello ghostty\\n'; exit 0",
    ]);
    let mut pane = Box::new(GhosttyPane::spawn(80, 24, cmd).expect("spawn failed"));

    for _ in 0..60 {
        if let AdvanceResult::Eof = pane.advance() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    let text = pane.read_screen_text();
    assert!(
        text.contains("hello ghostty"),
        "expected 'hello ghostty' in screen text, got: {text:?}"
    );
}

#[test]
fn feed_bytes_renders_to_screen() {
    let mut cmd = CommandBuilder::new("sleep");
    cmd.arg("10");
    let mut pane = GhosttyPane::spawn(80, 24, cmd).expect("spawn failed");

    // Feed bytes directly into the VT state machine (bypass PTY)
    pane.feed_bytes(b"direct feed test\r\n");

    let text = pane.read_screen_text();
    assert!(
        text.contains("direct feed test"),
        "expected 'direct feed test' in screen text, got: {text:?}"
    );
}

#[test]
fn exit_status_reports_success() {
    let mut pane = spawn_and_run("exit 0");
    // Child may not have fully exited yet — wait briefly.
    for _ in 0..20 {
        if pane.exit_status().is_some() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let exit = pane.exit_status();
    assert!(exit.is_some(), "expected exit status");
    let exit = exit.unwrap();
    assert!(
        exit.success(),
        "expected success, got code {}",
        exit.exit_code()
    );
}

#[test]
fn exit_status_reports_failure() {
    let mut pane = spawn_and_run("exit 42");
    let exit = pane.exit_status();
    assert!(exit.is_some(), "expected exit status");
    let exit = exit.unwrap();
    assert_eq!(exit.exit_code(), 42);
    assert!(!exit.success());
}

#[test]
fn read_screen_cells_has_content() {
    let pane = spawn_and_run("printf 'cells test\\n'; exit 0");
    let rows = pane.read_screen_cells(0);
    assert!(!rows.is_empty(), "expected non-empty screen cells");

    let all_text: String = rows
        .iter()
        .flat_map(|r| r.cells.iter().map(|c| c.text.as_str()))
        .collect();
    assert!(
        all_text.contains("cells test"),
        "expected 'cells test' in cells, got: {all_text:?}"
    );
}

#[test]
fn read_screen_cells_detects_bold() {
    let pane = spawn_and_run("printf '\\033[1mBOLD\\033[0m\\n'; exit 0");
    let rows = pane.read_screen_cells(0);
    let has_bold = rows
        .iter()
        .any(|r| r.cells.iter().any(|c| c.bold && !c.text.trim().is_empty()));
    assert!(has_bold, "expected at least one bold cell");
}

#[test]
fn read_screen_lines_range() {
    let pane = spawn_and_run("printf 'line1\\nline2\\nline3\\n'; exit 0");
    // Lines are 1-based: line1 is at row 1, line2 at row 2, line3 at row 3
    let range = pane.read_screen_lines("1-3", false);
    assert!(
        range.contains("line1") && range.contains("line2"),
        "expected range 1-3 to contain line1 and line2, got: {range:?}"
    );
    // Single line
    let single = pane.read_screen_lines("2", false);
    assert!(
        single.contains("line2"),
        "expected line 2 to contain 'line2', got: {single:?}"
    );
}

#[test]
fn search_scrollback_finds_match() {
    let pane = spawn_and_run("printf 'findme here\\n'; exit 0");
    let hits = pane.search_scrollback("findme");
    assert!(
        !hits.is_empty(),
        "expected at least one search hit for 'findme'"
    );
}

#[test]
fn search_scrollback_case_insensitive() {
    let pane = spawn_and_run("printf 'CamelCase\\n'; exit 0");
    let hits = pane.search_scrollback("camelcase");
    assert!(
        !hits.is_empty(),
        "expected case-insensitive match for 'camelcase'"
    );
}

#[test]
fn is_alive_while_running() {
    let mut cmd = CommandBuilder::new("sleep");
    cmd.arg("10");
    let mut pane = GhosttyPane::spawn(80, 24, cmd).expect("spawn failed");
    assert!(pane.is_alive(), "expected process to be alive");
}

#[test]
fn seqno_increments_on_advance() {
    let mut cmd = CommandBuilder::new("bash");
    cmd.args(["--norc", "--noprofile", "-c", "printf 'x\\n'; exit 0"]);
    let mut pane = GhosttyPane::spawn(80, 24, cmd).expect("spawn failed");

    let before = pane.current_seqno();
    for _ in 0..10 {
        if let AdvanceResult::Eof = pane.advance() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let after = pane.current_seqno();
    assert!(
        after > before,
        "expected seqno to increment: {before} -> {after}"
    );
}
