use std::io::Read;
use std::sync::Arc;
use std::thread;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};

use amux_term::config::AmuxTermConfig;
use amux_term::pane::TerminalPane;

/// Spawn `echo "hello from pty"` via a raw PTY and verify output bytes.
///
/// Uses a background reader thread because portable-pty readers block on macOS.
#[test]
fn echo_hello_from_pty() {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("failed to open pty");

    let mut cmd = CommandBuilder::new("echo");
    cmd.arg("hello from pty");
    let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("clone reader");

    let handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 4096];
        loop {
            match reader.read(&mut tmp) {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    });

    let _ = child.wait();
    drop(pair.master);

    let output = handle.join().expect("reader thread panicked");
    assert!(
        output.contains("hello from pty"),
        "expected 'hello from pty' in PTY output, got: {:?}",
        output
    );
}

/// Feed bytes directly to the terminal state machine and verify screen content.
/// This tests the wezterm-term integration without relying on PTY read timing.
#[test]
fn terminal_advance_bytes_renders_to_screen() {
    use wezterm_term::terminal::Terminal;
    use wezterm_term::TerminalSize;

    let config = Arc::new(AmuxTermConfig::default());
    let size = TerminalSize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
        dpi: 0,
    };

    // The writer is where the terminal sends responses (e.g. DA queries).
    // We don't need it for this test but must provide one.
    let writer: Box<dyn std::io::Write + Send> = Box::new(std::io::sink());
    let mut terminal = Terminal::new(size, config, "amux", "0.1.0", writer);

    // Feed some text as if it came from the PTY
    terminal.advance_bytes(b"hello from terminal\r\n");
    terminal.advance_bytes(b"second line\r\n");

    // Read the screen — get all lines including scrollback
    let screen = terminal.screen();
    let total = screen.scrollback_rows() + screen.physical_rows;
    let lines = screen.lines_in_phys_range(0..total);
    let text: String = lines
        .iter()
        .map(|l| l.as_str().to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        text.contains("hello from terminal"),
        "expected 'hello from terminal' in screen, got: {:?}",
        text
    );
    assert!(
        text.contains("second line"),
        "expected 'second line' in screen, got: {:?}",
        text
    );
}

/// Verify that terminal dimensions are reported correctly.
#[test]
fn dimensions_match_spawn_size() {
    let config = Arc::new(AmuxTermConfig::default());
    let cmd = CommandBuilder::new("echo");
    let pane = TerminalPane::spawn(120, 40, cmd, config).expect("failed to spawn pane");

    let (cols, rows) = pane.dimensions();
    assert_eq!(cols, 120);
    assert_eq!(rows, 40);
}

/// Verify that resize updates both terminal and PTY dimensions.
#[test]
fn resize_updates_dimensions() {
    let config = Arc::new(AmuxTermConfig::default());
    let cmd = CommandBuilder::new("echo");
    let mut pane = TerminalPane::spawn(80, 24, cmd, config).expect("failed to spawn pane");

    pane.resize(100, 30).expect("resize failed");

    let (cols, rows) = pane.dimensions();
    assert_eq!(cols, 100);
    assert_eq!(rows, 30);
}
