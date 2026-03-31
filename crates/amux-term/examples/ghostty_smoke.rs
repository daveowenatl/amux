//! Smoke test for the GhosttyPane backend.
//!
//! Spawns a shell, sends `echo hello && exit`, reads output, and prints
//! what the TerminalBackend trait reports.
//!
//! Run with:
//!   cargo run -p amux-term --features libghostty --example ghostty_smoke

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
        "echo 'hello from ghostty'; sleep 0.2; exit 42",
    ]);

    let mut pane = GhosttyPane::spawn(80, 24, cmd).expect("spawn failed");
    println!("Spawned. PID: {:?}", pane.child_pid());
    println!("Dimensions: {:?}", pane.dimensions());
    println!("Title: {:?}", pane.title());

    // Give the shell time to run
    for i in 0..20 {
        match pane.advance() {
            amux_term::pane::AdvanceResult::Read(n) => {
                println!("[tick {i}] Read {n} bytes");
            }
            amux_term::pane::AdvanceResult::WouldBlock => {}
            amux_term::pane::AdvanceResult::Eof => {
                println!("[tick {i}] EOF");
                break;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }

    println!("\n--- State after run ---");
    println!("Title: {:?}", pane.title());
    println!("Cursor: {:?}", pane.cursor());
    println!("Alt screen: {}", pane.is_alt_screen_active());
    println!("Bracketed paste: {}", pane.bracketed_paste_enabled());
    println!("Scrollback rows: {}", pane.scrollback_rows());
    println!("Alive: {}", pane.is_alive());
    println!("Seqno: {}", pane.current_seqno());

    let palette = pane.palette();
    println!(
        "Palette fg: ({:.2}, {:.2}, {:.2})",
        palette.foreground.0, palette.foreground.1, palette.foreground.2
    );
    println!(
        "Palette bg: ({:.2}, {:.2}, {:.2})",
        palette.background.0, palette.background.1, palette.background.2
    );
    println!("Palette colors: {} entries", palette.colors.len());

    if let Some(exit) = pane.exit_status() {
        println!("Exit code: {}", exit.exit_code());
        println!("Success: {}", exit.success());
    } else {
        println!("(process still running)");
    }

    println!("\n=== Done ===");
}
