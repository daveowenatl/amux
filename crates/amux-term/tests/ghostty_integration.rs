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

/// Helper: create a raw Terminal + RenderState pair for direct VT API tests.
fn new_terminal_and_render_state() -> (libghostty_vt::Terminal, libghostty_vt::RenderState) {
    let terminal = libghostty_vt::Terminal::new(libghostty_vt::TerminalOptions {
        cols: 80,
        rows: 24,
        max_scrollback: 10000,
    })
    .expect("terminal creation failed");
    let render_state = libghostty_vt::RenderState::new().expect("render state creation failed");
    (terminal, render_state)
}

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
    cmd.arg("1");
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
    cmd.arg("1");
    let mut pane = GhosttyPane::spawn(80, 24, cmd).expect("spawn failed");
    assert!(pane.is_alive(), "expected process to be alive");
}

#[test]
fn cursor_visible_after_show_hide_sequence() {
    let mut cmd = CommandBuilder::new("sleep");
    cmd.arg("1");
    let mut pane = GhosttyPane::spawn(80, 24, cmd).expect("spawn failed");

    // Cursor should be visible initially
    let cursor = pane.cursor();
    assert!(cursor.visible, "cursor should be visible initially");

    // Send DECTCEM hide: CSI ? 25 l
    pane.feed_bytes(b"\x1b[?25l");
    let cursor = pane.cursor();
    assert!(
        !cursor.visible,
        "cursor should be hidden after hide sequence"
    );

    // Send DECTCEM show: CSI ? 25 h
    pane.feed_bytes(b"\x1b[?25h");
    let cursor = pane.cursor();
    assert!(
        cursor.visible,
        "cursor should be visible after show sequence"
    );
}

#[test]
fn cursor_visible_after_rapid_toggle() {
    let mut cmd = CommandBuilder::new("sleep");
    cmd.arg("1");
    let mut pane = GhosttyPane::spawn(80, 24, cmd).expect("spawn failed");

    // Simulate rapid DECTCEM toggling like Claude Code does
    for _ in 0..50 {
        pane.feed_bytes(b"\x1b[?25l"); // hide
        pane.feed_bytes(b"\x1b[?25h"); // show
    }

    let cursor = pane.cursor();
    assert!(
        cursor.visible,
        "cursor should be visible after rapid show/hide toggling"
    );

    // Now test: hide, then do some output, then show
    pane.feed_bytes(b"\x1b[?25l");
    pane.feed_bytes(b"some output here\r\n");
    pane.feed_bytes(b"\x1b[?25h");

    let cursor = pane.cursor();
    assert!(
        cursor.visible,
        "cursor should be visible after hide-output-show pattern"
    );
}

#[test]
fn cursor_visible_raw_api_after_rapid_toggle() {
    // Test the raw libghostty-vt API directly, bypassing the workaround
    let (mut terminal, mut render_state) = new_terminal_and_render_state();

    // Initial state: cursor should be visible
    let snap = render_state.update(&terminal).expect("update failed");
    let visible = snap.cursor_visible().expect("cursor_visible failed");
    assert!(visible, "cursor should be visible initially (raw API)");

    // Simple hide
    terminal.vt_write(b"\x1b[?25l");
    let snap = render_state.update(&terminal).expect("update failed");
    let visible = snap.cursor_visible().expect("cursor_visible failed");
    assert!(!visible, "cursor should be hidden after hide (raw API)");

    // Simple show
    terminal.vt_write(b"\x1b[?25h");
    let snap = render_state.update(&terminal).expect("update failed");
    let visible = snap.cursor_visible().expect("cursor_visible failed");
    assert!(
        visible,
        "cursor should be visible after show (raw API) — FAILS if DECTCEM bug exists"
    );

    // Rapid toggling
    for _ in 0..100 {
        terminal.vt_write(b"\x1b[?25l");
        terminal.vt_write(b"\x1b[?25h");
    }
    let snap = render_state.update(&terminal).expect("update failed");
    let visible = snap.cursor_visible().expect("cursor_visible failed");
    assert!(
        visible,
        "cursor should be visible after 100 rapid toggles (raw API)"
    );

    // Rapid toggling with interleaved output (closer to Claude Code pattern)
    for i in 0..50 {
        terminal.vt_write(b"\x1b[?25l");
        let line = format!("line {i}\r\n");
        terminal.vt_write(line.as_bytes());
        terminal.vt_write(b"\x1b[?25h");
    }
    let snap = render_state.update(&terminal).expect("update failed");
    let visible = snap.cursor_visible().expect("cursor_visible failed");
    assert!(
        visible,
        "cursor should be visible after toggle-with-output pattern (raw API)"
    );

    // Batched writes — multiple sequences in one vt_write call
    for _ in 0..50 {
        terminal.vt_write(b"\x1b[?25lwriting stuff\r\n\x1b[?25h");
    }
    let snap = render_state.update(&terminal).expect("update failed");
    let visible = snap.cursor_visible().expect("cursor_visible failed");
    assert!(
        visible,
        "cursor should be visible after batched toggle-with-output (raw API)"
    );

    // Test: what does terminal.is_cursor_visible() say vs render state?
    let terminal_visible = terminal
        .is_cursor_visible()
        .expect("is_cursor_visible failed");
    assert_eq!(
        visible, terminal_visible,
        "render state and terminal should agree on cursor visibility"
    );
}

#[test]
fn cursor_visible_split_sequence() {
    // Test if splitting an escape sequence across two vt_write calls causes issues
    let (mut terminal, mut render_state) = new_terminal_and_render_state();

    // Hide cursor with complete sequence
    terminal.vt_write(b"\x1b[?25l");
    let snap = render_state.update(&terminal).expect("update failed");
    assert!(
        !snap.cursor_visible().expect("cursor_visible failed"),
        "cursor should be hidden"
    );

    // Show cursor with SPLIT sequence — ESC[ in one call, ?25h in another
    terminal.vt_write(b"\x1b[");
    terminal.vt_write(b"?25h");
    let snap = render_state.update(&terminal).expect("update failed");
    let visible = snap.cursor_visible().expect("cursor_visible failed");
    assert!(
        visible,
        "cursor should be visible after split show sequence — FAILS if split causes bug"
    );
}

#[test]
fn cursor_visible_truncated_in_buffer() {
    // Test if a truncated escape sequence at buffer boundary causes permanent state corruption
    let (mut terminal, mut render_state) = new_terminal_and_render_state();

    // Simulate a buffer read that cuts the escape sequence mid-way:
    // "\x1b[?25l" followed by text, then "\x1b[?25" cut off, then "h" in next buffer
    terminal.vt_write(b"\x1b[?25l");
    terminal.vt_write(b"some text\r\n");
    terminal.vt_write(b"\x1b[?25"); // truncated — missing 'h'
    terminal.vt_write(b"h"); // rest of sequence in next buffer

    let snap = render_state.update(&terminal).expect("update failed");
    let visible = snap.cursor_visible().expect("cursor_visible failed");
    assert!(
        visible,
        "cursor should be visible after truncated-then-completed sequence"
    );

    // Verify terminal agrees
    let terminal_visible = terminal
        .is_cursor_visible()
        .expect("is_cursor_visible failed");
    assert!(
        terminal_visible,
        "terminal should also report visible after truncated-then-completed sequence"
    );
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
