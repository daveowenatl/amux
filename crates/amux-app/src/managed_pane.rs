//! Managed pane types for the amux application.
//!
//! Currently all panes are terminal panes (`ManagedPane`). When a second
//! panel type is introduced (e.g., markdown viewer, browser), extract the
//! common interface into a `Panel` trait or enum:
//!
//! ```ignore
//! pub(crate) trait Panel {
//!     fn id(&self) -> PaneId;
//!     fn panel_type(&self) -> &'static str;
//!     fn title(&self) -> String;
//!     fn is_alive(&self) -> bool;
//!     fn panel_info(&self) -> PanelInfo;
//!     fn resize(&mut self, cols: u16, rows: u16);
//!     fn focus_changed(&mut self, focused: bool);
//! }
//! ```
//!
//! Then change `AmuxApp.panes` from `HashMap<PaneId, ManagedPane>`
//! to `HashMap<PaneId, Box<dyn Panel>>` (or an enum).

use std::sync::mpsc;

use amux_layout::PaneId;
use amux_term::pane::TerminalPane;

// ---------------------------------------------------------------------------
// Data Model
// ---------------------------------------------------------------------------
// Hierarchy: Workspace > PaneTree (splits) > Pane (each has tab bar) > Surface (terminal tab)

/// Per-surface metadata reported by shell integration, agent hooks, or OSC sequences.
#[derive(Default, Clone)]
pub(crate) struct SurfaceMetadata {
    pub(crate) cwd: Option<String>,
    pub(crate) git_branch: Option<String>,
    pub(crate) git_dirty: bool,
    pub(crate) pr_number: Option<u32>,
    pub(crate) pr_title: Option<String>,
    pub(crate) pr_state: Option<String>, // "open", "merged", "closed"
    /// Surface title from OSC 0/2 (window title set by shell/agent).
    pub(crate) surface_title: Option<String>,
}

/// Info about a process that has exited.
pub(crate) struct ExitInfo {
    pub(crate) message: String,
}

/// Action to take when user presses a key on a dead pane.
pub(crate) enum DeadPaneAction {
    None,
    Close,
    Restart,
}

/// A terminal tab within a pane. Each pane can have multiple surfaces.
pub(crate) struct PaneSurface {
    pub(crate) id: u64,
    pub(crate) pane: TerminalPane,
    pub(crate) byte_rx: mpsc::Receiver<Vec<u8>>,
    pub(crate) scroll_offset: usize,
    pub(crate) scroll_accum: f32,
    pub(crate) metadata: SurfaceMetadata,
    /// User-set title override. When set, takes precedence over OSC 0/2 title.
    pub(crate) user_title: Option<String>,
    /// Set when the PTY process exits.
    pub(crate) exited: Option<ExitInfo>,
}

/// A leaf in the split tree. Each pane has its own tab bar with surfaces.
pub(crate) struct ManagedPane {
    pub(crate) surfaces: Vec<PaneSurface>,
    pub(crate) active_surface_idx: usize,
    pub(crate) selection: Option<SelectionState>,
}

#[allow(dead_code)]
impl ManagedPane {
    pub(crate) fn active_surface(&self) -> &PaneSurface {
        &self.surfaces[self.active_surface_idx]
    }

    pub(crate) fn active_surface_mut(&mut self) -> &mut PaneSurface {
        &mut self.surfaces[self.active_surface_idx]
    }

    /// Display title for this pane (user title > OSC title > shell fallback).
    pub(crate) fn title(&self) -> String {
        let surface = self.active_surface();
        if let Some(ref t) = surface.user_title {
            return t.clone();
        }
        if let Some(ref t) = surface.metadata.surface_title {
            return t.clone();
        }
        surface.pane.title().to_string()
    }

    /// Whether the active surface's PTY process is still alive.
    pub(crate) fn is_alive(&mut self) -> bool {
        self.active_surface_mut().pane.is_alive()
    }

    /// Current dimensions (cols, rows) of the active surface.
    pub(crate) fn dimensions(&self) -> (usize, usize) {
        self.active_surface().pane.dimensions()
    }

    /// Panel type identifier for future multi-panel support.
    pub(crate) fn panel_type(&self) -> &'static str {
        amux_session::PANEL_TYPE_TERMINAL
    }

    /// Drain pending PTY output from the byte channel into the terminal state machine
    /// for all surfaces (not just the active one). Background tabs must keep their
    /// terminal state current so that titles, metadata, and scrollback stay in sync.
    /// Returns `true` if any bytes were processed (screen may need repaint).
    pub(crate) fn drain_pty_output(&mut self) -> bool {
        let mut any = false;
        for surface in &mut self.surfaces {
            while let Ok(bytes) = surface.byte_rx.try_recv() {
                surface.pane.feed_bytes(&bytes);
                any = true;
            }
        }
        any
    }

    /// Summary info for sidebar/IPC without exposing terminal internals.
    pub(crate) fn panel_info(&mut self) -> PanelInfo {
        PanelInfo {
            panel_type: self.panel_type(),
            title: self.title(),
            is_alive: self.is_alive(),
            surface_count: self.surfaces.len(),
        }
    }
}

/// Summary of a pane's state, usable without knowing the concrete panel type.
#[allow(dead_code)]
pub(crate) struct PanelInfo {
    pub(crate) panel_type: &'static str,
    pub(crate) title: String,
    pub(crate) is_alive: bool,
    pub(crate) surface_count: usize,
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionMode {
    Cell,
    Word,
    Line,
}

#[derive(Debug, Clone)]
pub(crate) struct SelectionState {
    pub(crate) anchor: (usize, usize), // (col, stable_row)
    pub(crate) end: (usize, usize),    // (col, stable_row)
    pub(crate) mode: SelectionMode,
    pub(crate) active: bool, // true while mouse is held
}

impl SelectionState {
    /// Return (start, end) normalized so start <= end in reading order.
    pub(crate) fn normalized(&self) -> ((usize, usize), (usize, usize)) {
        let a = self.anchor;
        let b = self.end;
        if a.1 < b.1 || (a.1 == b.1 && a.0 <= b.0) {
            (a, b)
        } else {
            (b, a)
        }
    }

    /// Check if a cell at (col, stable_row) is within the selection.
    pub(crate) fn contains(&self, col: usize, stable_row: usize) -> bool {
        let (start, end) = self.normalized();
        if stable_row < start.1 || stable_row > end.1 {
            return false;
        }
        if start.1 == end.1 {
            // Single line
            col >= start.0 && col <= end.0
        } else if stable_row == start.1 {
            col >= start.0
        } else if stable_row == end.1 {
            col <= end.0
        } else {
            true // middle line
        }
    }
}

// ---------------------------------------------------------------------------
// Find / Copy Mode
// ---------------------------------------------------------------------------

/// State for the in-pane find/search bar.
pub(crate) struct FindState {
    pub(crate) query: String,
    /// Matches as (phys_row, start_col, end_col_exclusive).
    pub(crate) matches: Vec<(usize, usize, usize)>,
    pub(crate) current_match: usize,
    /// The pane this search applies to.
    pub(crate) pane_id: PaneId,
    /// True on the first frame after opening, for initial focus.
    pub(crate) just_opened: bool,
}

/// State for vi-style copy mode (scrollback navigation + visual selection).
pub(crate) struct CopyModeState {
    pub(crate) pane_id: PaneId,
    /// Cursor position in (col, phys_row).
    pub(crate) cursor: (usize, usize),
    /// Visual selection anchor (col, phys_row), set when 'v' is pressed.
    pub(crate) visual_anchor: Option<(usize, usize)>,
    /// Line-visual mode (V).
    pub(crate) line_visual: bool,
}

/// Word boundary delimiters for double-click selection.
pub(crate) const WORD_DELIMITERS: &str = " \t\n()[]{}'\"|<>&;:,.`~!@#$%^*-+=?/\\";
