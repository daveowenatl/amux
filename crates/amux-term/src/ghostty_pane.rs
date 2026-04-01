//! libghostty-vt backend for the TerminalBackend trait.
//!
//! POC implementation (#34) proving that `TerminalBackend` can be implemented
//! against a non-wezterm backend. Wraps libghostty-vt's Terminal + portable-pty.
//!
//! Enabled with `--features libghostty`.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::sync::{mpsc, Arc, Mutex};

use libghostty_vt::render::{CellIterator, CursorVisualStyle, Dirty, RenderState, RowIterator};
use libghostty_vt::style::RgbColor;
use libghostty_vt::terminal::{Mode, Options as TerminalOptions, Terminal};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use url::Url;

use crate::backend::{
    Color, CursorPos, CursorShape, Palette, ProcessExit, ScreenCell, ScreenRow, StableRow,
    TerminalBackend,
};
use crate::osc::NotificationEvent;
use crate::pane::{AdvanceResult, SequenceNo, TermError};

/// A terminal pane backed by libghostty-vt + portable-pty.
///
/// libghostty-vt is !Send + !Sync, so this must stay on one thread.
/// `RenderState` is behind `RefCell` for interior mutability — the trait's
/// `&self` methods need to take snapshots for screen reading.
pub struct GhosttyPane<'alloc, 'cb> {
    /// Boxed so the vtable pointer registered via `on_pty_write` etc. remains
    /// stable when GhosttyPane is moved (into AnyBackend, PaneSurface, etc.).
    /// libghostty-vt stores `&self.vtable` as a raw C pointer; moving Terminal
    /// after callback registration would leave a dangling pointer → SIGSEGV.
    terminal: Box<Terminal<'alloc, 'cb>>,
    render_state: RefCell<RenderState<'alloc>>,
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
    /// Cached cursor shape from last render state update.
    cached_cursor_shape: CursorShape,
    /// Cached working directory URL (from OSC 7 via pwd()).
    cached_working_dir: Option<Url>,
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
        // Box the terminal BEFORE registering callbacks. libghostty-vt stores
        // a raw pointer to Terminal.vtable via ghostty_terminal_set(USERDATA, &self.vtable).
        // Boxing first ensures the vtable has a stable heap address that survives
        // moves of GhosttyPane into AnyBackend/PaneSurface.
        let mut terminal = Box::new(
            Terminal::new(opts).map_err(|e| TermError::PtySetupFailed(anyhow::anyhow!("{e}")))?,
        );

        // Register on_pty_write callback so terminal responses go back to PTY.
        let write_handle = Arc::clone(&shared);
        terminal
            .on_pty_write(move |_term, data| {
                let mut w = write_handle.lock().unwrap_or_else(|e| e.into_inner());
                if let Err(e) = w.write_all(data) {
                    tracing::warn!("PTY write failed: {e}");
                }
            })
            .map_err(|e| TermError::PtySetupFailed(anyhow::anyhow!("{e}")))?;

        // Register bell callback for notifications.
        let (tx, rx) = mpsc::channel();
        let bell_tx = tx.clone();
        terminal
            .on_bell(move |_term| {
                let _ = bell_tx.send(NotificationEvent::Bell);
            })
            .map_err(|e| TermError::PtySetupFailed(anyhow::anyhow!("{e}")))?;

        // Register title change callback.
        let title_tx = tx;
        terminal
            .on_title_changed(move |term| {
                if let Ok(title) = term.title() {
                    let _ = title_tx.send(NotificationEvent::TitleChanged(title.to_string()));
                }
            })
            .map_err(|e| TermError::PtySetupFailed(anyhow::anyhow!("{e}")))?;

        let render_state =
            RenderState::new().map_err(|e| TermError::PtySetupFailed(anyhow::anyhow!("{e}")))?;

        Ok(Self {
            terminal,
            render_state: RefCell::new(render_state),
            master: pair.master,
            child,
            reader: Some(reader),
            writer: shared,
            seqno: 0,
            rendered_seqno: 0,
            notification_rx: rx,
            cached_palette: Palette::default(),
            cached_cursor_shape: CursorShape::Default,
            cached_working_dir: None,
        })
    }

    /// Refresh cached render state (palette, cursor shape, working dir).
    /// Called after vt_write to keep caches warm.
    fn refresh_render_cache(&mut self) {
        let mut rs = self.render_state.borrow_mut();
        if let Ok(snapshot) = rs.update(&self.terminal) {
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
                self.cached_cursor_shape = cursor_style_to_shape(style);
            }
        }

        // Cache working directory from OSC 7
        if let Ok(pwd) = self.terminal.pwd() {
            if !pwd.is_empty() {
                self.cached_working_dir = Url::parse(pwd)
                    .ok()
                    .or_else(|| Url::parse(&format!("file://{pwd}")).ok());
            }
        }
    }

    /// Read all visible lines from a render state snapshot.
    /// Returns a Vec of lines (trimmed on the right).
    fn snapshot_lines(&self) -> Vec<String> {
        let mut rs = self.render_state.borrow_mut();
        let snapshot = match rs.update(&self.terminal) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let mut row_iter = match RowIterator::new() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        let mut cell_iter = match CellIterator::new() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let mut row_iteration = match row_iter.update(&snapshot) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        let mut lines = Vec::new();
        while let Some(row) = row_iteration.next() {
            let mut line = String::new();
            if let Ok(mut cell_iteration) = cell_iter.update(row) {
                while let Some(cell) = cell_iteration.next() {
                    if let Ok(chars) = cell.graphemes() {
                        for ch in chars {
                            line.push(ch);
                        }
                    }
                }
            }
            lines.push(line.trim_end().to_string());
        }
        lines
    }

    /// Read screen text using the render state snapshot iterators.
    /// Takes &self via RefCell interior mutability.
    fn read_text_from_snapshot(&self) -> String {
        let mut lines = self.snapshot_lines();
        // Trim trailing empty lines
        while lines.last().is_some_and(|l| l.is_empty()) {
            lines.pop();
        }
        lines.join("\n")
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
        self.cached_working_dir.as_ref()
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
        if self.seqno > self.rendered_seqno {
            let rows = self.terminal.rows().unwrap_or(24) as i64;
            (0..rows).collect()
        } else {
            Vec::new()
        }
    }

    fn mark_rendered(&mut self) {
        self.rendered_seqno = self.seqno;
        let mut rs = self.render_state.borrow_mut();
        if let Ok(snapshot) = rs.update(&self.terminal) {
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

    fn read_screen_lines(&self, line_spec: &str, _ansi: bool) -> String {
        // ANSI formatting not yet supported — returns plain text regardless.
        // Uses the render state snapshot (viewport lines).
        let lines = self.snapshot_lines();
        let total = lines.len();
        if total == 0 {
            return String::new();
        }

        let (start, end) = if let Some(rest) = line_spec.strip_prefix('-') {
            // "-N" means last N lines
            let n: usize = rest.parse().unwrap_or(total);
            (total.saturating_sub(n), total)
        } else if let Some((a, b)) = line_spec.split_once('-') {
            // "A-B" means lines A through B (1-based)
            let a: usize = a.parse().unwrap_or(1);
            let b: usize = b.parse().unwrap_or(total);
            let s = a.saturating_sub(1).min(total);
            let e = b.min(total);
            if s >= e {
                (0, 0)
            } else {
                (s, e)
            }
        } else {
            // Single line number (1-based)
            let n: usize = line_spec.parse().unwrap_or(1);
            let idx = n.saturating_sub(1).min(total.saturating_sub(1));
            (idx, (idx + 1).min(total))
        };

        let mut selected: Vec<&str> = lines[start..end].iter().map(|s| s.as_str()).collect();
        // Trim trailing empty lines
        while selected.last().is_some_and(|l| l.is_empty()) {
            selected.pop();
        }
        selected.join("\n")
    }

    fn read_screen_text(&self) -> String {
        self.read_text_from_snapshot()
    }

    fn read_scrollback_text(&self, max_lines: usize) -> String {
        // libghostty-vt 0.1.x only exposes viewport via render state.
        // PointCoordinate fields are private, so grid_ref can't reach scrollback.
        let lines = self.snapshot_lines();
        if max_lines > lines.len() {
            tracing::warn!(
                "read_scrollback_text({max_lines}) requested but only {} viewport lines available \
                 (ghostty backend cannot read scrollback history)",
                lines.len()
            );
        }
        let total = lines.len();
        let start = total.saturating_sub(max_lines);
        let mut selected: Vec<&str> = lines[start..].iter().map(|s| s.as_str()).collect();
        while selected.last().is_some_and(|l| l.is_empty()) {
            selected.pop();
        }
        selected.join("\n")
    }

    fn read_scrollback_text_range(&self, start: usize, end: usize) -> String {
        // libghostty-vt 0.1.x only exposes viewport via render state.
        let lines = self.snapshot_lines();
        let viewport_total = self.terminal.total_rows().unwrap_or(0);
        let viewport_rows = lines.len();
        let viewport_start = viewport_total.saturating_sub(viewport_rows);

        if start < viewport_start || end > viewport_total {
            tracing::warn!(
                "read_scrollback_text_range({start}..{end}) extends beyond viewport \
                 ({viewport_start}..{viewport_total}), results may be incomplete \
                 (ghostty backend cannot read scrollback history)"
            );
        }

        // Map physical row range to viewport-relative indices.
        let s = start.saturating_sub(viewport_start).min(viewport_rows);
        let e = end.saturating_sub(viewport_start).min(viewport_rows);
        if s >= e {
            return String::new();
        }
        let mut selected: Vec<&str> = lines[s..e].iter().map(|s| s.as_str()).collect();
        while selected.last().is_some_and(|l| l.is_empty()) {
            selected.pop();
        }
        selected.join("\n")
    }

    fn search_scrollback(&self, query: &str) -> Vec<(usize, usize, usize)> {
        if query.is_empty() {
            return Vec::new();
        }
        let query_lower = query.to_lowercase();
        let lines = self.snapshot_lines();

        // Map physical row indices to account for viewport offset.
        let total = self.terminal.total_rows().unwrap_or(0);
        let viewport_rows = lines.len();
        let viewport_start = total.saturating_sub(viewport_rows);

        let mut matches = Vec::new();
        for (viewport_idx, line) in lines.iter().enumerate() {
            let line_lower = line.to_lowercase();
            let mut search_start = 0;
            while let Some(byte_pos) = line_lower[search_start..].find(&query_lower) {
                let abs_byte = search_start + byte_pos;
                let end_byte = abs_byte + query_lower.len();
                // Convert byte offsets to character (column) offsets for
                // correct highlighting with multi-byte UTF-8 content.
                let start_col = line_lower[..abs_byte].chars().count();
                let match_chars = line_lower[abs_byte..end_byte].chars().count();
                let phys_row = viewport_start + viewport_idx;
                matches.push((phys_row, start_col, start_col + match_chars));
                search_start = abs_byte + 1;
            }
        }
        matches
    }

    fn read_screen_cells(&self, _scroll_offset: usize) -> Vec<ScreenRow> {
        let mut rs = self.render_state.borrow_mut();
        let snapshot = match rs.update(&self.terminal) {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };

        let palette = self.cached_palette.clone();
        let default_fg = palette.foreground;
        let default_bg = palette.background;

        let mut row_iter = match RowIterator::new() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        let mut cell_iter = match CellIterator::new() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let mut row_iteration = match row_iter.update(&snapshot) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        let mut rows = Vec::new();
        while let Some(row) = row_iteration.next() {
            let wrapped = row
                .raw_row()
                .map(|r| r.is_wrapped().unwrap_or(false))
                .unwrap_or(false);
            let mut cells = Vec::new();
            if let Ok(mut cell_iteration) = cell_iter.update(row) {
                while let Some(cell) = cell_iteration.next() {
                    let text = cell
                        .graphemes()
                        .map(|chars| chars.into_iter().collect::<String>())
                        .unwrap_or_default();

                    let fg = cell
                        .fg_color()
                        .ok()
                        .flatten()
                        .map(rgb_to_color)
                        .unwrap_or(default_fg);
                    let bg = cell
                        .bg_color()
                        .ok()
                        .flatten()
                        .map(rgb_to_color)
                        .unwrap_or(default_bg);

                    let style = cell.style().ok();
                    let bold = style.as_ref().map(|s| s.bold).unwrap_or(false);
                    let italic = style.as_ref().map(|s| s.italic).unwrap_or(false);
                    let underline = style
                        .as_ref()
                        .map(|s| !matches!(s.underline, libghostty_vt::style::Underline::None))
                        .unwrap_or(false);
                    let strikethrough = style.as_ref().map(|s| s.strikethrough).unwrap_or(false);
                    let inverse = style.as_ref().map(|s| s.inverse).unwrap_or(false);

                    cells.push(ScreenCell {
                        text,
                        fg,
                        bg,
                        bold,
                        italic,
                        underline,
                        strikethrough,
                        reverse: inverse,
                        hyperlink_url: None,
                    });
                }
            }
            rows.push(ScreenRow { cells, wrapped });
        }
        rows
    }

    fn read_cells_range(&self, start_row: usize, end_row: usize) -> Vec<ScreenRow> {
        // libghostty-vt's render state only exposes the viewport. grid_ref
        // supports arbitrary Screen coordinates but PointCoordinate fields
        // are private in 0.1.x, so we can't use it.
        // Return the viewport rows that overlap the requested range.
        let total = self.terminal.total_rows().unwrap_or(0);
        let viewport_rows = self.terminal.rows().unwrap_or(24) as usize;
        let viewport_start = total.saturating_sub(viewport_rows);
        let viewport_end = total;

        // No overlap — requested range is entirely in scrollback history.
        if end_row <= viewport_start || start_row >= viewport_end {
            tracing::debug!(
                "read_cells_range({start_row}..{end_row}) outside viewport \
                 ({viewport_start}..{viewport_end}), returning empty"
            );
            return Vec::new();
        }

        let all_rows = self.read_screen_cells(0);

        // Map physical row range to viewport-relative indices.
        let rel_start = start_row.saturating_sub(viewport_start);
        let rel_end = (end_row - viewport_start).min(all_rows.len());
        if rel_start >= rel_end {
            return Vec::new();
        }
        all_rows[rel_start..rel_end].to_vec()
    }

    fn erase_scrollback(&mut self) {
        // Send CSI 3 J (Erase Scrollback) to clear scrollback without
        // resetting the terminal. terminal.reset() would wipe screen too.
        self.terminal.vt_write(b"\x1b[3J");
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

fn cursor_style_to_shape(style: CursorVisualStyle) -> CursorShape {
    match style {
        CursorVisualStyle::Bar => CursorShape::SteadyBar,
        CursorVisualStyle::Block => CursorShape::SteadyBlock,
        CursorVisualStyle::Underline => CursorShape::SteadyUnderline,
        CursorVisualStyle::BlockHollow => CursorShape::SteadyBlock,
        _ => CursorShape::Default,
    }
}
