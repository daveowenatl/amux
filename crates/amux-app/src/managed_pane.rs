//! Managed pane types for the amux application.
//!
//! `PaneEntry` is the top-level enum stored in `AmuxApp.panes`. Terminal and
//! browser tabs are stored in a single ordered `Vec<TabEntry>` within a
//! `ManagedPane`, allowing arbitrary interleaving and insert-after-active.

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
// Tab entry — a single tab in the unified tab list
// ---------------------------------------------------------------------------

/// A tab in a ManagedPane's tab bar. Can be either a terminal surface or a
/// reference to a browser pane in the global panes map.
pub(crate) enum TabEntry {
    Terminal(Box<PaneSurface>),
    Browser(PaneId),
}

impl TabEntry {
    /// Returns true if this is a browser tab.
    pub(crate) fn is_browser(&self) -> bool {
        matches!(self, TabEntry::Browser(_))
    }

    /// Returns the browser PaneId if this is a browser tab.
    pub(crate) fn browser_pane_id(&self) -> Option<PaneId> {
        match self {
            TabEntry::Browser(id) => Some(*id),
            TabEntry::Terminal(_) => None,
        }
    }

    /// Returns a reference to the terminal surface if this is a terminal tab.
    pub(crate) fn as_surface(&self) -> Option<&PaneSurface> {
        match self {
            TabEntry::Terminal(s) => Some(s),
            TabEntry::Browser(_) => None,
        }
    }

    /// Returns a mutable reference to the terminal surface if this is a terminal tab.
    pub(crate) fn as_surface_mut(&mut self) -> Option<&mut PaneSurface> {
        match self {
            TabEntry::Terminal(s) => Some(s),
            TabEntry::Browser(_) => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Active tab kind
// ---------------------------------------------------------------------------

/// Which kind of tab is active in a ManagedPane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActiveTab {
    /// A terminal surface at the given index in `tabs`.
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

impl PaneSurface {
    /// Snap the viewport to the bottom (most recent output).
    /// For backends that manage their own scroll (e.g., libghostty), this
    /// also tells the backend to jump to the bottom.
    pub(crate) fn snap_scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        self.scroll_accum = 0.0;
        if self.pane.manages_own_scroll() {
            self.pane.scroll_to_bottom();
        }
    }
}

/// A leaf in the split tree. Each pane has its own tab bar with
/// terminal surfaces and browser tabs in a single ordered list.
pub(crate) struct ManagedPane {
    pub(crate) tabs: Vec<TabEntry>,
    pub(crate) active_tab_idx: usize,
    pub(crate) selection: Option<SelectionState>,
}

#[allow(dead_code)]
impl ManagedPane {
    /// Total number of tabs.
    pub(crate) fn tab_count(&self) -> usize {
        self.tabs.len()
    }

    /// What kind of tab is currently active.
    pub(crate) fn active_tab(&self) -> ActiveTab {
        match &self.tabs[self.active_tab_idx] {
            TabEntry::Terminal(_) => ActiveTab::Terminal(self.active_tab_idx),
            TabEntry::Browser(id) => ActiveTab::Browser(*id),
        }
    }

    /// Whether the active tab is a browser tab.
    pub(crate) fn active_is_browser(&self) -> bool {
        self.tabs[self.active_tab_idx].is_browser()
    }

    /// Returns the active terminal surface.
    /// When a browser tab is active, returns the nearest terminal surface as a
    /// safe fallback (callers should check `active_is_browser()` first).
    ///
    /// # Panics
    /// Panics if there are no terminal surfaces.
    pub(crate) fn active_surface(&self) -> &PaneSurface {
        if let TabEntry::Terminal(s) = &self.tabs[self.active_tab_idx] {
            return s;
        }
        // Fallback: find the last terminal surface
        self.tabs
            .iter()
            .filter_map(|t| t.as_surface())
            .next_back()
            .expect("active_surface() called with no terminal surfaces")
    }

    /// Returns the active terminal surface mutably.
    /// When a browser tab is active, returns the last terminal surface as a
    /// safe fallback (callers should check `active_is_browser()` first).
    ///
    /// # Panics
    /// Panics if there are no terminal surfaces.
    pub(crate) fn active_surface_mut(&mut self) -> &mut PaneSurface {
        // Try active tab first
        let idx = if matches!(self.tabs[self.active_tab_idx], TabEntry::Terminal(_)) {
            self.active_tab_idx
        } else {
            // Fallback: find last terminal tab index
            self.tabs
                .iter()
                .rposition(|t| matches!(t, TabEntry::Terminal(_)))
                .expect("active_surface_mut() called with no terminal surfaces")
        };
        match &mut self.tabs[idx] {
            TabEntry::Terminal(s) => s,
            TabEntry::Browser(_) => unreachable!(),
        }
    }

    /// Collect all browser PaneIds referenced by this pane's tabs.
    pub(crate) fn browser_pane_ids(&self) -> Vec<PaneId> {
        self.tabs
            .iter()
            .filter_map(|t| t.browser_pane_id())
            .collect()
    }

    /// Iterator over all terminal surfaces.
    pub(crate) fn surfaces(&self) -> impl Iterator<Item = &PaneSurface> {
        self.tabs.iter().filter_map(|t| t.as_surface())
    }

    /// Mutable iterator over all terminal surfaces.
    pub(crate) fn surfaces_mut(&mut self) -> impl Iterator<Item = &mut PaneSurface> {
        self.tabs.iter_mut().filter_map(|t| t.as_surface_mut())
    }

    /// Display title for this pane (user title > OSC title > shell fallback).
    pub(crate) fn title(&self) -> String {
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
            return (80, 24);
        }
        self.active_surface().pane.dimensions()
    }

    /// Panel type identifier for future multi-panel support.
    pub(crate) fn panel_type(&self) -> &'static str {
        amux_session::PANEL_TYPE_TERMINAL
    }

    /// Drain pending PTY output from the byte channel into the terminal state machine
    /// for all terminal surfaces (not just the active one). Background tabs must keep
    /// their terminal state current so titles, metadata, and scrollback stay in sync.
    /// Returns `true` if any bytes were processed (screen may need repaint).
    pub(crate) fn drain_pty_output(&mut self) -> bool {
        let mut any = false;
        for surface in self.surfaces_mut() {
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
