//! Managed pane types for the amux application.
//!
//! `PaneEntry` is the top-level enum stored in `AmuxApp.panes`. Terminal and
//! browser surfaces share the same tab bar within a `ManagedPane`, matching
//! the cmux UX where browser tabs sit alongside terminal tabs.

use std::sync::mpsc;

use amux_layout::PaneId;
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
#[allow(dead_code)]
pub(crate) enum PaneEntry {
    Terminal(ManagedPane),
    Browser(amux_browser::BrowserPane),
}

#[allow(dead_code)]
impl PaneEntry {
    /// Returns a reference to the inner `ManagedPane` if this is a terminal pane.
    pub(crate) fn as_terminal(&self) -> Option<&ManagedPane> {
        match self {
            PaneEntry::Terminal(m) => Some(m),
            PaneEntry::Browser(_) => None,
        }
    }

    /// Returns a mutable reference to the inner `ManagedPane` if this is a terminal pane.
    pub(crate) fn as_terminal_mut(&mut self) -> Option<&mut ManagedPane> {
        match self {
            PaneEntry::Terminal(m) => Some(m),
            PaneEntry::Browser(_) => None,
        }
    }

    /// Returns a reference to the inner `BrowserPane` if this is a browser pane.
    pub(crate) fn as_browser(&self) -> Option<&amux_browser::BrowserPane> {
        match self {
            PaneEntry::Browser(b) => Some(b),
            PaneEntry::Terminal(_) => None,
        }
    }

    /// Returns a mutable reference to the inner `BrowserPane` if this is a browser pane.
    #[allow(dead_code)]
    pub(crate) fn as_browser_mut(&mut self) -> Option<&mut amux_browser::BrowserPane> {
        match self {
            PaneEntry::Browser(b) => Some(b),
            PaneEntry::Terminal(_) => None,
        }
    }

    /// Display title for this pane, dispatched by type.
    pub(crate) fn title(&self) -> String {
        match self {
            PaneEntry::Terminal(m) => m.title(),
            PaneEntry::Browser(b) => {
                let t = b.title();
                if t.is_empty() {
                    b.url().unwrap_or_else(|| "Browser".to_string())
                } else {
                    t.to_string()
                }
            }
        }
    }

    /// Panel type identifier string.
    pub(crate) fn panel_type(&self) -> &'static str {
        match self {
            PaneEntry::Terminal(m) => m.panel_type(),
            PaneEntry::Browser(_) => "browser",
        }
    }

    /// Summary info for sidebar/IPC without exposing internals.
    pub(crate) fn panel_info(&mut self) -> PanelInfo {
        match self {
            PaneEntry::Terminal(m) => m.panel_info(),
            PaneEntry::Browser(b) => PanelInfo {
                panel_type: "browser",
                title: {
                    let t = b.title();
                    if t.is_empty() {
                        b.url().unwrap_or_else(|| "Browser".to_string())
                    } else {
                        t.to_string()
                    }
                },
                is_alive: true,
                surface_count: 1,
            },
        }
    }

    /// Whether this is a browser pane.
    pub(crate) fn is_browser(&self) -> bool {
        matches!(self, PaneEntry::Browser(_))
    }
}

// ---------------------------------------------------------------------------
// Tab kind — unified tab index for terminal + browser tabs
// ---------------------------------------------------------------------------

/// Which kind of tab is active in a ManagedPane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActiveTab {
    /// A terminal surface at the given index in `surfaces`.
    Terminal(usize),
    /// A browser tab — the PaneId references a `PaneEntry::Browser` in the panes map.
    Browser(PaneId),
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

/// A leaf in the split tree. Each pane has its own tab bar with both
/// terminal surfaces and browser tabs.
pub(crate) struct ManagedPane {
    pub(crate) surfaces: Vec<PaneSurface>,
    /// Browser pane IDs that appear as tabs in this pane's tab bar.
    /// Each ID references a `PaneEntry::Browser` in the main panes map.
    pub(crate) browser_tab_ids: Vec<PaneId>,
    pub(crate) active_surface_idx: usize,
    pub(crate) selection: Option<SelectionState>,
}

#[allow(dead_code)]
impl ManagedPane {
    /// Total number of tabs (terminal surfaces + browser tabs).
    pub(crate) fn tab_count(&self) -> usize {
        self.surfaces.len() + self.browser_tab_ids.len()
    }

    /// What kind of tab is currently active.
    pub(crate) fn active_tab(&self) -> ActiveTab {
        if self.active_surface_idx < self.surfaces.len() {
            ActiveTab::Terminal(self.active_surface_idx)
        } else {
            let browser_idx = self.active_surface_idx - self.surfaces.len();
            ActiveTab::Browser(self.browser_tab_ids[browser_idx])
        }
    }

    /// Whether the active tab is a browser tab.
    pub(crate) fn active_is_browser(&self) -> bool {
        self.active_surface_idx >= self.surfaces.len()
    }

    /// Returns the active terminal surface.
    /// When a browser tab is active, returns the last terminal surface as a
    /// safe fallback (callers should check `active_is_browser()` first).
    pub(crate) fn active_surface(&self) -> &PaneSurface {
        let idx = self.active_surface_idx.min(self.surfaces.len() - 1);
        &self.surfaces[idx]
    }

    /// Returns the active terminal surface mutably.
    /// When a browser tab is active, returns the last terminal surface as a
    /// safe fallback (callers should check `active_is_browser()` first).
    pub(crate) fn active_surface_mut(&mut self) -> &mut PaneSurface {
        let idx = self.active_surface_idx.min(self.surfaces.len() - 1);
        &mut self.surfaces[idx]
    }

    /// Display title for this pane (user title > OSC title > shell fallback).
    pub(crate) fn title(&self) -> String {
        // If active tab is a browser, title comes from the browser pane
        // (handled by caller since we don't have access to the panes map here).
        if self.active_is_browser() {
            return "Browser".to_string();
        }
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
        if self.active_is_browser() {
            return true;
        }
        self.active_surface_mut().pane.is_alive()
    }

    /// Current dimensions (cols, rows) of the active surface.
    pub(crate) fn dimensions(&self) -> (usize, usize) {
        if self.active_is_browser() {
            return (80, 24); // placeholder for browser tabs
        }
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
            surface_count: self.tab_count(),
        }
    }
}
