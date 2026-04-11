//! `TerminalPaintCallback` — the per-pane egui paint callback handle.
//!
//! This struct carries the terminal snapshot and physical rect for a single
//! pane into egui's render pass. The `CallbackTrait` implementation that
//! actually walks the snapshot, shapes glyphs, and emits GPU instances lives
//! in `screen_line.rs`.

use crate::snapshot::TerminalSnapshot;
use crate::state::PhysRect;

/// Paint callback for a single terminal pane.
pub struct TerminalPaintCallback {
    pub pane_id: u64,
    pub snapshot: TerminalSnapshot,
    pub phys_rect: PhysRect,
    pub cell_width: f32,
    pub cell_height: f32,
}
