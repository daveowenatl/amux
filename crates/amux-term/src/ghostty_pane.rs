//! libghostty-vt backend for the TerminalBackend trait.
//!
//! POC implementation (#34) proving that `TerminalBackend` can be implemented
//! against a non-wezterm backend. Wraps libghostty-vt's Terminal + portable-pty.
//!
//! Enabled with `--features libghostty`.

use std::io::{Read, Write};
use std::sync::{mpsc, Arc, Mutex};

use libghostty_vt::render::{Dirty, RenderState};
use libghostty_vt::style::RgbColor;
use libghostty_vt::terminal::{Mode, Options as TerminalOptions, Terminal};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use url::Url;

use crate::backend::{
    Color, CursorPos, CursorShape, Palette, ProcessExit, StableRow, TerminalBackend,
};
use crate::osc::NotificationEvent;
use crate::pane::{AdvanceResult, SequenceNo, TermError};

/// A terminal pane backed by libghostty-vt + portable-pty.
///
/// libghostty-vt is !Send + !Sync, so this must stay on one thread.
pub struct GhosttyPane<'alloc, 'cb> {
    terminal: Terminal<'alloc, 'cb>,
    render_state: RenderState<'alloc>,
    #[allow(dead_code)]
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    reader: Option<Box<dyn Read + Send>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    seqno: SequenceNo,
    rendered_seqno: SequenceNo,
    notification_rx: mpsc::Receiver<NotificationEvent>,
    /// Cached palette from last render state update.
    cached_palette: Palette,
    /// Cached cursor from last render state update.
    cached_cursor_shape: CursorShape,
}

impl<'alloc, 'cb> GhosttyPane<'alloc, 'cb>
where
    'alloc: 'cb,
{
    /// Spawn a new terminal pane running the given command.
    pub fn spawn(cols: u16, rows: u16, cmd: CommandBuilder) -> Result<Self, TermError> {
        let pty_system = native_pty_system();
        let pty_size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let pair = pty_system
            .openpty(pty_size)
            .map_err(TermError::PtySetupFailed)?;

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(TermError::PtySetupFailed)?;
        let reader = pair
            .master
            .try_clone_reader()
            .map_err(TermError::PtySetupFailed)?;

        let pty_writer = pair
            .master
            .take_writer()
            .map_err(TermError::PtySetupFailed)?;
        let shared = Arc::new(Mutex::new(pty_writer));

        let opts = TerminalOptions {
            cols,
            rows,
            max_scrollback: 10_000,
        };
        let mut terminal =
            Terminal::new(opts).map_err(|e| TermError::PtySetupFailed(anyhow::anyhow!("{e}")))?;

        // Register on_pty_write callback so terminal responses go back to PTY.
        let write_handle = Arc::clone(&shared);
        terminal
            .on_pty_write(move |_term, data| {
                let mut w = write_handle.lock().unwrap_or_else(|e| e.into_inner());
                let _ = w.write_all(data);
            })
            .map_err(|e| TermError::PtySetupFailed(anyhow::anyhow!("{e}")))?;

        let render_state =
            RenderState::new().map_err(|e| TermError::PtySetupFailed(anyhow::anyhow!("{e}")))?;

        let (_tx, rx) = mpsc::channel();

        Ok(Self {
            terminal,
            render_state,
            master: pair.master,
            child,
            reader: Some(reader),
            writer: shared,
            seqno: 0,
            rendered_seqno: 0,
            notification_rx: rx,
            cached_palette: Palette::default(),
            cached_cursor_shape: CursorShape::Default,
        })
    }

    /// Refresh cached render state (palette, cursor shape).
    /// Called after vt_write to keep caches warm.
    fn refresh_render_cache(&mut self) {
        if let Ok(snapshot) = self.render_state.update(&self.terminal) {
            // Cache palette
            if let Ok(colors) = snapshot.colors() {
                let fg = rgb_to_color(colors.foreground);
                let bg = rgb_to_color(colors.background);
                let cursor_color = colors.cursor.map(rgb_to_color).unwrap_or(fg);
                self.cached_palette = Palette {
                    foreground: fg,
                    background: bg,
                    cursor_fg: bg,
                    cursor_bg: cursor_color,
                    cursor_border: cursor_color,
                    selection_fg: Color::BLACK,
                    selection_bg: Color(0.4, 0.6, 1.0, 1.0),
                    colors: colors.palette.iter().map(|&c| rgb_to_color(c)).collect(),
                };
            }

            // Cache cursor shape
            if let Ok(style) = snapshot.cursor_visual_style() {
                use libghostty_vt::render::CursorVisualStyle;
                self.cached_cursor_shape = match style {
                    CursorVisualStyle::Bar => CursorShape::SteadyBar,
                    CursorVisualStyle::Block => CursorShape::SteadyBlock,
                    CursorVisualStyle::Underline => CursorShape::SteadyUnderline,
                    CursorVisualStyle::BlockHollow => CursorShape::SteadyBlock,
                    _ => CursorShape::Default,
                };
            }
        }
    }
}

impl TerminalBackend for GhosttyPane<'_, '_> {
    fn advance(&mut self) -> AdvanceResult {
        let reader = match &mut self.reader {
            Some(r) => r,
            None => return AdvanceResult::Eof,
        };
        let mut buf = [0u8; 8192];
        match reader.read(&mut buf) {
            Ok(0) => AdvanceResult::Eof,
            Ok(n) => {
                self.terminal.vt_write(&buf[..n]);
                self.seqno += 1;
                self.refresh_render_cache();
                AdvanceResult::Read(n)
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => AdvanceResult::WouldBlock,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => AdvanceResult::WouldBlock,
            Err(_) => AdvanceResult::Eof,
        }
    }

    fn resize(&mut self, cols: u16, rows: u16) -> Result<(), TermError> {
        let pty_size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        self.master
            .resize(pty_size)
            .map_err(TermError::ResizeFailed)?;
        self.terminal
            .resize(cols, rows, 0, 0)
            .map_err(|e| TermError::ResizeFailed(anyhow::anyhow!("{e}")))?;
        Ok(())
    }

    fn write_bytes(&mut self, data: &[u8]) -> Result<(), TermError> {
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        writer.write_all(data).map_err(TermError::WriteFailed)?;
        Ok(())
    }

    fn feed_bytes(&mut self, data: &[u8]) {
        self.terminal.vt_write(data);
        self.seqno += 1;
        self.refresh_render_cache();
    }

    fn take_reader(&mut self) -> Option<Box<dyn Read + Send>> {
        self.reader.take()
    }

    fn title(&self) -> &str {
        self.terminal.title().unwrap_or("?")
    }

    fn working_dir(&self) -> Option<&Url> {
        // libghostty returns &str from pwd(). We'd need a cached Url field
        // updated via an on_osc7 callback. For the POC, return None.
        None
    }

    fn dimensions(&self) -> (usize, usize) {
        let cols = self.terminal.cols().unwrap_or(80) as usize;
        let rows = self.terminal.rows().unwrap_or(24) as usize;
        (cols, rows)
    }

    fn cursor(&self) -> CursorPos {
        CursorPos {
            x: self.terminal.cursor_x().unwrap_or(0) as usize,
            y: self.terminal.cursor_y().unwrap_or(0) as i64,
            shape: self.cached_cursor_shape,
            visible: self.terminal.is_cursor_visible().unwrap_or(true),
        }
    }

    fn palette(&self) -> Palette {
        self.cached_palette.clone()
    }

    fn is_alt_screen_active(&self) -> bool {
        use libghostty_vt::ffi::GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_ALTERNATE;
        self.terminal
            .active_screen()
            .map(|s| s == GhosttyTerminalScreen_GHOSTTY_TERMINAL_SCREEN_ALTERNATE)
            .unwrap_or(false)
    }

    fn bracketed_paste_enabled(&self) -> bool {
        self.terminal.mode(Mode::BRACKETED_PASTE).unwrap_or(false)
    }

    fn child_pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn exit_status(&mut self) -> Option<ProcessExit> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.into()),
            _ => None,
        }
    }

    fn changed_lines(&self) -> Vec<StableRow> {
        // libghostty tracks dirty rows via RenderState. For the POC,
        // report all visible rows when seqno has advanced.
        if self.seqno > self.rendered_seqno {
            let rows = self.terminal.rows().unwrap_or(24) as i64;
            (0..rows).collect()
        } else {
            Vec::new()
        }
    }

    fn mark_rendered(&mut self) {
        self.rendered_seqno = self.seqno;
        // Reset dirty tracking
        if let Ok(snapshot) = self.render_state.update(&self.terminal) {
            let _ = snapshot.set_dirty(Dirty::Clean);
        }
    }

    fn current_seqno(&self) -> SequenceNo {
        self.seqno
    }

    fn rendered_seqno(&self) -> SequenceNo {
        self.rendered_seqno
    }

    fn scrollback_rows(&self) -> usize {
        self.terminal.total_rows().unwrap_or(0)
    }

    fn read_screen_lines(&self, _line_spec: &str, _ansi: bool) -> String {
        // POC: full implementation would parse line_spec and iterate scrollback.
        // For now, return empty — the IPC layer will get basic functionality.
        String::new()
    }

    fn read_screen_text(&self) -> String {
        // This needs &mut self for render_state.update(), but the trait requires &self.
        // POC limitation: return empty. A full implementation would maintain a cached
        // screen text updated in advance()/feed_bytes().
        String::new()
    }

    fn read_scrollback_text(&self, _max_lines: usize) -> String {
        String::new()
    }

    fn read_scrollback_text_range(&self, _start: usize, _end: usize) -> String {
        String::new()
    }

    fn search_scrollback(&self, _query: &str) -> Vec<(usize, usize, usize)> {
        Vec::new()
    }

    fn erase_scrollback(&mut self) {
        self.terminal.reset();
    }

    fn focus_changed(&mut self, focused: bool) {
        let event = if focused {
            libghostty_vt::focus::Event::Gained
        } else {
            libghostty_vt::focus::Event::Lost
        };
        let mut buf = [0u8; 8];
        if let Ok(n) = event.encode(&mut buf) {
            self.terminal.vt_write(&buf[..n]);
        }
    }

    fn drain_notifications(&self) -> Vec<NotificationEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.notification_rx.try_recv() {
            events.push(event);
        }
        events
    }
}

fn rgb_to_color(rgb: RgbColor) -> Color {
    Color(
        rgb.r as f32 / 255.0,
        rgb.g as f32 / 255.0,
        rgb.b as f32 / 255.0,
        1.0,
    )
}
