//! Managed pane types for the amux application.
//!
//! `PaneEntry` is the top-level enum stored in `AmuxApp.panes`. Currently the
//! only variant is `Terminal`, but this enum is the extension point for
//! browser panes and other panel types.

use std::sync::mpsc;

use amux_term::AnyBackend;
use amux_term::TerminalBackend;

// Re-export core model types so existing `use managed_pane::*` keeps working.
pub(crate) use amux_core::model::{
    CopyModeState, DeadPaneAction, ExitInfo, FindState, PanelInfo, SelectionMode, SelectionState,
    SurfaceMetadata, WORD_DELIMITERS,
};

// ---------------------------------------------------------------------------
// Pane entry enum — the value type of `AmuxApp.panes`
// ---------------------------------------------------------------------------

/// A pane entry in the application's pane map.
///
/// Terminal panes are the only variant today. When browser panes land (#108),
/// add `Browser(BrowserPane)` here and the compiler will flag every call site
/// that needs a new match arm.
pub(crate) enum PaneEntry {
    Terminal(ManagedPane),
}

#[allow(dead_code)]
impl PaneEntry {
    /// Returns a reference to the inner `ManagedPane` if this is a terminal pane.
    pub(crate) fn as_terminal(&self) -> Option<&ManagedPane> {
        match self {
            PaneEntry::Terminal(m) => Some(m),
        }
    }

    /// Returns a mutable reference to the inner `ManagedPane` if this is a terminal pane.
    pub(crate) fn as_terminal_mut(&mut self) -> Option<&mut ManagedPane> {
        match self {
            PaneEntry::Terminal(m) => Some(m),
        }
    }

    /// Display title for this pane, dispatched by type.
    pub(crate) fn title(&self) -> String {
        match self {
            PaneEntry::Terminal(m) => m.title(),
        }
    }

    /// Panel type identifier string.
    pub(crate) fn panel_type(&self) -> &'static str {
        match self {
            PaneEntry::Terminal(m) => m.panel_type(),
        }
    }

    /// Summary info for sidebar/IPC without exposing internals.
    pub(crate) fn panel_info(&mut self) -> PanelInfo {
        match self {
            PaneEntry::Terminal(m) => m.panel_info(),
        }
    }
}

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
