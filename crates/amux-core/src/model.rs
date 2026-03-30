//! Core data model types shared between amux-app and amux-cli.
//!
//! These types represent the application's domain model without any
//! GUI framework dependency. Types that depend on terminal internals
//! (e.g. `PaneSurface`, `ManagedPane`) remain in amux-app.

use std::collections::HashMap;

use amux_layout::{PaneId, PaneTree, SplitDirection};

// ---------------------------------------------------------------------------
// Workspace / Sidebar
// ---------------------------------------------------------------------------

/// A workspace shown in the sidebar. Owns the split tree.
pub struct Workspace {
    pub id: u64,
    pub title: String,
    pub tree: PaneTree,
    pub focused_pane: PaneId,
    pub zoomed: Option<PaneId>,
    pub dragging_divider: Option<DragState>,
    pub last_pane_sizes: HashMap<PaneId, (usize, usize)>,
    /// Optional workspace color for sidebar indicator.
    pub color: Option<[u8; 4]>,
}

pub struct SidebarState {
    pub visible: bool,
    pub width: f32,
    /// Drag reorder state.
    pub drag: Option<SidebarDragState>,
}

pub struct SidebarDragState {
    /// Index of workspace being dragged.
    pub source_idx: usize,
    /// Current pointer Y position.
    pub current_y: f32,
    /// Computed drop target index.
    pub drop_target_idx: usize,
    /// Y midpoints of each row for computing drop position.
    pub row_midpoints: Vec<f32>,
}

pub struct DragState {
    pub node_path: Vec<usize>,
    pub direction: SplitDirection,
}

// ---------------------------------------------------------------------------
// Surface metadata
// ---------------------------------------------------------------------------

/// Per-surface metadata reported by shell integration, agent hooks, or OSC sequences.
#[derive(Default, Clone)]
pub struct SurfaceMetadata {
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
    pub git_dirty: bool,
    pub pr_number: Option<u32>,
    pub pr_title: Option<String>,
    pub pr_state: Option<String>, // "open", "merged", "closed"
    /// Surface title from OSC 0/2 (window title set by shell/agent).
    pub surface_title: Option<String>,
}

/// Info about a process that has exited.
pub struct ExitInfo {
    pub message: String,
}

/// Action to take when user presses a key on a dead pane.
pub enum DeadPaneAction {
    None,
    Close,
    Restart,
}

/// Summary of a pane's state, usable without knowing the concrete panel type.
#[allow(dead_code)]
pub struct PanelInfo {
    pub panel_type: &'static str,
    pub title: String,
    pub is_alive: bool,
    pub surface_count: usize,
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionMode {
    Cell,
    Word,
    Line,
}

#[derive(Debug, Clone)]
pub struct SelectionState {
    pub anchor: (usize, usize), // (col, stable_row)
    pub end: (usize, usize),    // (col, stable_row)
    pub mode: SelectionMode,
    pub active: bool, // true while mouse is held
}

impl SelectionState {
    /// Return (start, end) normalized so start <= end in reading order.
    pub fn normalized(&self) -> ((usize, usize), (usize, usize)) {
        let a = self.anchor;
        let b = self.end;
        if a.1 < b.1 || (a.1 == b.1 && a.0 <= b.0) {
            (a, b)
        } else {
            (b, a)
        }
    }

    /// Check if a cell at (col, stable_row) is within the selection.
    pub fn contains(&self, col: usize, stable_row: usize) -> bool {
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
pub struct FindState {
    pub query: String,
    /// Matches as (phys_row, start_col, end_col_exclusive).
    pub matches: Vec<(usize, usize, usize)>,
    pub current_match: usize,
    /// The pane this search applies to.
    pub pane_id: PaneId,
    /// True on the first frame after opening, for initial focus.
    pub just_opened: bool,
}

/// State for vi-style copy mode (scrollback navigation + visual selection).
pub struct CopyModeState {
    pub pane_id: PaneId,
    /// Cursor position in (col, phys_row).
    pub cursor: (usize, usize),
    /// Visual selection anchor (col, phys_row), set when 'v' is pressed.
    pub visual_anchor: Option<(usize, usize)>,
    /// Line-visual mode (V).
    pub line_visual: bool,
}

/// Word boundary delimiters for double-click selection.
pub const WORD_DELIMITERS: &str = " \t\n()[]{}'\"|<>&;:,.`~!@#$%^*-+=?/\\";
