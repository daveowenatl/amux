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

use amux_term::AnyBackend;
use amux_term::TerminalBackend;

// Re-export core model types so existing `use managed_pane::*` keeps working.
pub(crate) use amux_core::model::{
    CopyModeState, DeadPaneAction, ExitInfo, FindState, PanelInfo, SelectionMode, SelectionState,
    SurfaceMetadata, WORD_DELIMITERS,
};

// ---------------------------------------------------------------------------
// Terminal-dependent types (stay in amux-app)
// ---------------------------------------------------------------------------

/// A terminal tab within a pane. Each pane can have multiple surfaces.
pub(crate) struct PaneSurface {
    pub(crate) id: u64,
    pub(crate) pane: AnyBackend,
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
