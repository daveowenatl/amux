//! GPU-side resources stored in egui's `CallbackResources` for the terminal renderer.
//!
//! `TerminalGpuResources` owns the pipelines, glyph atlas, font system, per-pane
//! render states, image cache, and shape cache. A single instance lives for the
//! lifetime of the application and is shared across all terminal paint callbacks
//! via `egui_wgpu`'s callback resource map.

use std::collections::{HashMap, HashSet};

use amux_term::font::DecorationMetrics;
use cosmic_text::{FontSystem, Metrics, SwashCache};
use egui_wgpu::wgpu;

use crate::atlas::GlyphAtlas;
use crate::pipeline::{BackgroundPipeline, ForegroundPipeline, ImagePipeline};
use crate::state::{ImageTextureEntry, PaneRenderState, ShapeCacheKey, ShapedGlyphEntry};

/// Resources stored in egui's `CallbackResources` for the terminal renderer.
pub struct TerminalGpuResources {
    pub bg_pipeline: BackgroundPipeline,
    pub fg_pipeline: ForegroundPipeline,
    pub img_pipeline: ImagePipeline,
    pub atlas: GlyphAtlas,
    pub font_system: FontSystem,
    pub swash_cache: SwashCache,
    pub metrics: Metrics,
    pub decoration_metrics: DecorationMetrics,
    pub atlas_bind_group_dirty: bool,
    /// Whether the render target uses an sRGB format. When true, colors must
    /// be converted to linear space because the hardware applies linear→sRGB
    /// on store. When false (e.g., Bgra8Unorm on macOS), sRGB values are
    /// passed through directly.
    pub target_is_srgb: bool,
    /// Per-pane render state (instance buffers + dirty tracking).
    pub pane_states: HashMap<u64, PaneRenderState>,
    /// Cached GPU textures for inline images, keyed by image hash.
    pub image_cache: HashMap<[u8; 32], ImageTextureEntry>,
    /// Shape cache: maps (text, bold, italic) → shaped glyph data.
    /// Avoids re-running cosmic-text shaping for previously seen glyphs.
    pub shape_cache: HashMap<ShapeCacheKey, Vec<ShapedGlyphEntry>>,
    /// Shared sampler for image textures.
    pub image_sampler: wgpu::Sampler,
    /// Cached curly underline atlas tile (one cell-width sine wave).
    pub curly_tile: Option<crate::atlas::AtlasEntry>,
    /// Cached dotted underline atlas tile (row of circles).
    pub dotted_tile: Option<crate::atlas::AtlasEntry>,
    /// Last pixels_per_point used for shape cache and decoration tiles.
    /// When DPI changes, these caches are invalidated.
    pub last_pixels_per_point: f32,
}

impl TerminalGpuResources {
    /// Remove render state for panes that are no longer alive.
    pub fn retain_panes(&mut self, live_pane_ids: &[u64]) {
        let live: HashSet<u64> = live_pane_ids.iter().copied().collect();
        self.pane_states.retain(|id, _| live.contains(id));
    }

    /// Remove image textures not referenced by any current pane's image draws.
    pub fn evict_unused_images(&mut self) {
        let mut referenced: HashSet<[u8; 32]> = HashSet::new();
        for state in self.pane_states.values() {
            for (hash, _, _) in &state.image_draws {
                referenced.insert(*hash);
            }
        }
        self.image_cache.retain(|hash, _| referenced.contains(hash));
    }
}
