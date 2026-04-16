//! libghostty-vt backend for the TerminalBackend trait.
//!
//! Wraps libghostty-vt's Terminal + portable-pty. This is the sole terminal
//! engine used by amux.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::sync::{mpsc, Arc, Mutex};

use libghostty_vt::key as gkey;
use libghostty_vt::render::{CellIterator, CursorVisualStyle, Dirty, RenderState, RowIterator};
use libghostty_vt::style::RgbColor;
use libghostty_vt::terminal::{Mode, Options as TerminalOptions, Terminal};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use url::Url;

use crate::backend::{
    AdvanceResult, Color, CursorPos, CursorShape, Palette, ProcessExit, ScreenCell, ScreenRow,
    SequenceNo, StableRow, TermError, TerminalBackend, UnderlineStyle,
};
use crate::osc::NotificationEvent;

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
    /// When true, an external palette was set via `set_palette()` and
    /// `refresh_render_cache` should not overwrite it with libghostty defaults.
    palette_overridden: bool,
    /// Cached cursor shape from last render state update.
    cached_cursor_shape: CursorShape,
    /// Cached working directory URL (from OSC 7 via pwd()).
    cached_working_dir: Option<Url>,
    /// libghostty key encoder — reused across calls, configured from terminal
    /// state on each encode so it picks up Kitty keyboard protocol flags.
    key_encoder: gkey::Encoder<'alloc>,
}

impl<'alloc, 'cb> GhosttyPane<'alloc, 'cb>
where
    'alloc: 'cb,
{
    /// Spawn a new terminal pane running the given command.
    pub fn spawn(
        cols: u16,
        rows: u16,
        cmd: CommandBuilder,
        max_scrollback: usize,
    ) -> Result<Self, TermError> {
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
            max_scrollback,
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

        let key_encoder = gkey::Encoder::new()
            .map_err(|e| TermError::PtySetupFailed(anyhow::anyhow!("key encoder: {e}")))?;

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
            palette_overridden: false,
            cached_cursor_shape: CursorShape::Default,
            cached_working_dir: None,
            key_encoder,
        })
    }

    /// Override the cached palette with colors from amux's theme.
    /// Called once after construction so the ghostty backend uses amux's
    /// configured colors instead of libghostty-vt's built-in defaults.
    pub fn set_palette(&mut self, palette: Palette) {
        self.cached_palette = palette;
        self.palette_overridden = true;
    }

    /// Refresh cached render state (palette, cursor shape, working dir).
    /// Called after vt_write to keep caches warm.
    fn refresh_render_cache(&mut self) {
        let mut rs = self.render_state.borrow_mut();
        if let Ok(snapshot) = rs.update(&self.terminal) {
            // Cache palette (skip if externally overridden by amux theme).
            if !self.palette_overridden {
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
            }

            // Cache cursor shape (combining visual style + blink flag)
            if let Ok(style) = snapshot.cursor_visual_style() {
                let blinking = snapshot.cursor_blinking().unwrap_or(false);
                self.cached_cursor_shape = cursor_style_to_shape(style, blinking);
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

    /// Return the set of row indices (in viewport order) whose semantic_prompt
    /// state is Prompt or Continuation. Requires the shell integration to emit
    /// OSC 133 marks for populated data; returns empty set otherwise.
    fn prompt_row_indices(&self) -> std::collections::HashSet<usize> {
        use libghostty_vt::screen::RowSemanticPrompt;
        let mut result = std::collections::HashSet::new();
        let mut rs = self.render_state.borrow_mut();
        let Ok(snapshot) = rs.update(&self.terminal) else {
            return result;
        };
        let Ok(mut row_iter) = RowIterator::new() else {
            return result;
        };
        let Ok(mut row_iteration) = row_iter.update(&snapshot) else {
            return result;
        };
        let mut idx: usize = 0;
        while let Some(row) = row_iteration.next() {
            if let Ok(raw) = row.raw_row() {
                if let Ok(state) = raw.semantic_prompt() {
                    if !matches!(state, RowSemanticPrompt::None) {
                        result.insert(idx);
                    }
                }
            }
            idx += 1;
        }
        result
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
            let mut col: usize = 0; // cell index (always +1 per cell)
            let mut visible_col: usize = 0; // cells actually written
            if let Ok(mut cell_iteration) = cell_iter.update(row) {
                while let Some(cell) = cell_iteration.next() {
                    if let Ok(chars) = cell.graphemes() {
                        if !chars.is_empty() {
                            // Pad with spaces up to this column if we skipped blank cells.
                            while visible_col < col {
                                line.push(' ');
                                visible_col += 1;
                            }
                            for ch in chars {
                                line.push(ch);
                            }
                            visible_col += 1;
                        }
                    }
                    col += 1;
                }
            }
            lines.push(line.trim_end().to_string());
        }
        lines
    }

    /// Build lines with ANSI SGR escape sequences for color/style preservation.
    /// Used by `read_scrollback_text` so session restore retains styling.
    fn snapshot_lines_ansi(&self) -> Vec<String> {
        use libghostty_vt::style::Underline as GhosttyUnderline;

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

        let default_style = libghostty_vt::style::Style::default();

        let mut lines = Vec::new();
        while let Some(row) = row_iteration.next() {
            let mut line = String::new();
            let mut col: usize = 0;
            let mut visible_col: usize = 0;
            let mut prev_style = default_style;
            let mut prev_fg: Option<RgbColor> = None;
            let mut prev_bg: Option<RgbColor> = None;

            if let Ok(mut cell_iteration) = cell_iter.update(row) {
                while let Some(cell) = cell_iteration.next() {
                    let graphemes = cell.graphemes().unwrap_or_default();
                    if graphemes.is_empty() {
                        col += 1;
                        continue;
                    }

                    // Pad with spaces up to this column.
                    while visible_col < col {
                        line.push(' ');
                        visible_col += 1;
                    }

                    // Check if style changed.
                    let cur_style = cell.style().unwrap_or(default_style);
                    let cur_fg = cell.fg_color().ok().flatten();
                    let cur_bg = cell.bg_color().ok().flatten();

                    let style_changed =
                        cur_style != prev_style || cur_fg != prev_fg || cur_bg != prev_bg;

                    if style_changed {
                        // Reset, then emit active attributes.
                        line.push_str("\x1b[0");

                        if cur_style.bold {
                            line.push_str(";1");
                        }
                        if cur_style.faint {
                            line.push_str(";2");
                        }
                        if cur_style.italic {
                            line.push_str(";3");
                        }
                        match cur_style.underline {
                            GhosttyUnderline::Single => line.push_str(";4"),
                            GhosttyUnderline::Double => line.push_str(";21"),
                            GhosttyUnderline::Curly => line.push_str(";4:3"),
                            GhosttyUnderline::Dotted => line.push_str(";4:4"),
                            GhosttyUnderline::Dashed => line.push_str(";4:5"),
                            _ => {}
                        }
                        if cur_style.inverse {
                            line.push_str(";7");
                        }
                        if cur_style.invisible {
                            line.push_str(";8");
                        }
                        if cur_style.strikethrough {
                            line.push_str(";9");
                        }

                        // Foreground color
                        {
                            use std::fmt::Write;
                            if let Some(fg) = cur_fg {
                                let _ = write!(line, ";38;2;{};{};{}", fg.r, fg.g, fg.b);
                            }

                            // Background color
                            if let Some(bg) = cur_bg {
                                let _ = write!(line, ";48;2;{};{};{}", bg.r, bg.g, bg.b);
                            }

                            // Underline color (SGR 58;2;R;G;B)
                            if !matches!(cur_style.underline, GhosttyUnderline::None) {
                                if let Some(uc) = resolve_style_color(
                                    &cur_style.underline_color,
                                    &self.cached_palette.colors,
                                ) {
                                    let _ = write!(
                                        line,
                                        ";58;2;{};{};{}",
                                        (uc.0 * 255.0) as u8,
                                        (uc.1 * 255.0) as u8,
                                        (uc.2 * 255.0) as u8
                                    );
                                }
                            }
                        }

                        line.push('m');

                        prev_style = cur_style;
                        prev_fg = cur_fg;
                        prev_bg = cur_bg;
                    }

                    for ch in graphemes {
                        line.push(ch);
                    }
                    col += 1;
                    visible_col += 1;
                }
            }

            // Reset at end of line if we had non-default attributes.
            if prev_style != default_style || prev_fg.is_some() || prev_bg.is_some() {
                line.push_str("\x1b[0m");
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
        // Bump seqno so the renderer takes a fresh snapshot at the new
        // dimensions on the next render. Don't call refresh_render_cache()
        // here — the Zig VT engine may not be in a consistent state for
        // snapshotting immediately after resize (STATUS_BREAKPOINT crash
        // on Windows). The renderer will snapshot during the subsequent
        // render pass when it sees the bumped seqno.
        self.seqno += 1;
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
        // Use the render state snapshot for cursor position — it provides
        // viewport-relative coordinates that account for scrollback.
        let mut rs = self.render_state.borrow_mut();
        if let Ok(snapshot) = rs.update(&self.terminal) {
            if let Ok(Some(vp)) = snapshot.cursor_viewport() {
                let visible = snapshot.cursor_visible().unwrap_or(true);
                return CursorPos {
                    x: vp.x as usize,
                    y: vp.y as i64,
                    shape: self.cached_cursor_shape,
                    visible,
                };
            }
        }
        CursorPos {
            x: 0,
            y: 0,
            shape: self.cached_cursor_shape,
            visible: false,
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
        let lines = self.snapshot_lines_ansi();
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

        // Drop trailing blank rows. Then drop at most one trailing prompt
        // row (the unused prompt that's visible when save fires). We can't
        // aggressively drop all trailing prompt rows because on Windows the
        // pwsh integration can't emit OSC 133;B/C (no preexec hook), so
        // ghostty tags command output rows as Prompt/Continuation too —
        // an unconditional drop would eat the content we're trying to save.
        let prompt_row_indices = self.prompt_row_indices();
        while let Some(last_idx) = selected.len().checked_sub(1) {
            if selected[last_idx].trim().is_empty() {
                selected.pop();
            } else {
                break;
            }
        }
        if let Some(last_idx) = selected.len().checked_sub(1) {
            let row_idx = start + last_idx;
            if prompt_row_indices.contains(&row_idx) {
                selected.pop();
            }
        }

        selected.join("\n")
    }

    fn read_scrollback_text_range(&self, start: usize, end: usize) -> String {
        // libghostty-vt 0.1.x only exposes viewport via render state.
        let lines = self.snapshot_lines_ansi();
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
                        .map(|s| match s.underline {
                            libghostty_vt::style::Underline::None => UnderlineStyle::None,
                            libghostty_vt::style::Underline::Single => UnderlineStyle::Single,
                            libghostty_vt::style::Underline::Double => UnderlineStyle::Double,
                            libghostty_vt::style::Underline::Curly => UnderlineStyle::Curly,
                            libghostty_vt::style::Underline::Dotted => UnderlineStyle::Dotted,
                            libghostty_vt::style::Underline::Dashed => UnderlineStyle::Dashed,
                            _ => UnderlineStyle::Single,
                        })
                        .unwrap_or(UnderlineStyle::None);
                    let strikethrough = style.as_ref().map(|s| s.strikethrough).unwrap_or(false);
                    let faint = style.as_ref().map(|s| s.faint).unwrap_or(false);
                    let inverse = style.as_ref().map(|s| s.inverse).unwrap_or(false);

                    let underline_color = style.as_ref().and_then(|s| {
                        resolve_style_color(&s.underline_color, &self.cached_palette.colors)
                    });
                    cells.push(ScreenCell {
                        text,
                        fg,
                        bg,
                        bold,
                        italic,
                        underline,
                        underline_color,
                        strikethrough,
                        faint,
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

    fn manages_own_scroll(&self) -> bool {
        true
    }

    fn scroll_viewport(&mut self, delta: isize) {
        use libghostty_vt::terminal::ScrollViewport;
        self.terminal.scroll_viewport(ScrollViewport::Delta(delta));
        // Bump seqno so the GPU renderer's dirty check triggers a redraw.
        self.seqno += 1;
    }

    fn scroll_to_bottom(&mut self) {
        use libghostty_vt::terminal::ScrollViewport;
        self.terminal.scroll_viewport(ScrollViewport::Bottom);
        self.seqno += 1;
    }

    fn erase_scrollback(&mut self) {
        // Send CSI 3 J (Erase Scrollback) to clear scrollback without
        // touching the visible screen. terminal.reset() / RIS would wipe
        // the screen too. There's no visible change after this — the
        // scrollback is just gone if the user tries to scroll up.
        self.terminal.vt_write(b"\x1b[3J");
        // Bump seqno so the renderer's dirty fingerprint detects the
        // VT state change (scrollback row count dropped) and re-snapshots.
        self.seqno += 1;
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

    fn encode_key(
        &mut self,
        key: gkey::Key,
        mods: gkey::Mods,
        action: gkey::Action,
        text: Option<&str>,
        unshifted_codepoint: Option<char>,
    ) -> Option<Vec<u8>> {
        // Sync encoder options (Kitty flags, DECCKM, etc.) from terminal state.
        self.key_encoder.set_options_from_terminal(&self.terminal);

        #[cfg(target_os = "macos")]
        self.key_encoder
            .set_macos_option_as_alt(gkey::OptionAsAlt::True);

        let mut event = gkey::Event::new().ok()?;
        event.set_key(key);
        event.set_mods(mods);
        event.set_action(action);
        event.set_utf8(text);
        if let Some(cp) = unshifted_codepoint {
            event.set_unshifted_codepoint(cp);
        }

        let mut buf = Vec::with_capacity(32);
        match self.key_encoder.encode_to_vec(&event, &mut buf) {
            Ok(()) if !buf.is_empty() => Some(buf),
            _ => None,
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

/// Resolve a ghostty `StyleColor` to an amux `Color`, or `None` if unset.
fn resolve_style_color(sc: &libghostty_vt::style::StyleColor, palette: &[Color]) -> Option<Color> {
    match sc {
        libghostty_vt::style::StyleColor::None => None,
        libghostty_vt::style::StyleColor::Rgb(rgb) => Some(rgb_to_color(*rgb)),
        libghostty_vt::style::StyleColor::Palette(idx) => palette.get(idx.0 as usize).copied(),
    }
}

fn cursor_style_to_shape(style: CursorVisualStyle, blinking: bool) -> CursorShape {
    match (style, blinking) {
        (CursorVisualStyle::Bar, true) => CursorShape::BlinkingBar,
        (CursorVisualStyle::Bar, false) => CursorShape::SteadyBar,
        (CursorVisualStyle::Block, true) => CursorShape::BlinkingBlock,
        (CursorVisualStyle::Block, false) => CursorShape::SteadyBlock,
        (CursorVisualStyle::Underline, true) => CursorShape::BlinkingUnderline,
        (CursorVisualStyle::Underline, false) => CursorShape::SteadyUnderline,
        (CursorVisualStyle::BlockHollow, _) => CursorShape::SteadyBlock,
        _ => CursorShape::Default,
    }
}
