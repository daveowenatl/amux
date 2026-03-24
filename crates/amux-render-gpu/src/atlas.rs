use std::collections::HashMap;

use cosmic_text::{CacheKey, FontSystem, SwashCache, SwashContent, SwashImage};
use egui_wgpu::wgpu;

/// A single glyph's location in the atlas texture.
#[derive(Clone, Copy)]
pub struct AtlasEntry {
    /// UV coordinates in the atlas: [u_min, v_min, u_max, v_max] in 0..1 range.
    pub uv: [f32; 4],
    /// Glyph placement offset from cell origin (in pixels).
    pub placement_left: i32,
    pub placement_top: i32,
    /// Glyph pixel dimensions.
    pub width: u32,
    pub height: u32,
}

/// Shelf in the shelf-packing algorithm.
struct Shelf {
    y: u32,
    height: u32,
    x_cursor: u32,
}

/// GPU glyph atlas using shelf-packing.
///
/// Rasterizes glyphs via cosmic-text/swash and uploads them to a GPU texture.
/// Currently uses a single R8Unorm texture for monochrome glyphs.
pub struct GlyphAtlas {
    texture: wgpu::Texture,
    pub texture_view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    size: u32,
    shelves: Vec<Shelf>,
    cache: HashMap<CacheKey, Option<AtlasEntry>>,
}

impl GlyphAtlas {
    /// Create a new glyph atlas with the given texture size.
    pub fn new(device: &wgpu::Device, size: u32) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("glyph_atlas"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glyph_atlas_sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Self {
            texture,
            texture_view,
            sampler,
            size,
            shelves: Vec::new(),
            cache: HashMap::new(),
        }
    }

    /// Look up or rasterize a glyph, returning its atlas entry.
    ///
    /// Returns `None` for glyphs that can't be rasterized (e.g., spaces, missing glyphs).
    pub fn get_or_insert(
        &mut self,
        queue: &wgpu::Queue,
        font_system: &mut FontSystem,
        swash_cache: &mut SwashCache,
        cache_key: CacheKey,
    ) -> Option<AtlasEntry> {
        if let Some(entry) = self.cache.get(&cache_key) {
            return *entry;
        }

        let image: Option<SwashImage> = swash_cache.get_image_uncached(font_system, cache_key);

        let entry = image.and_then(|img| {
            if img.placement.width == 0 || img.placement.height == 0 {
                return None;
            }

            // Only handle monochrome glyphs for now (color emoji in PR 8f).
            let alpha_data = match img.content {
                SwashContent::Mask => img.data.clone(),
                SwashContent::Color => {
                    // Extract alpha channel from RGBA data.
                    img.data.iter().skip(3).step_by(4).copied().collect()
                }
                SwashContent::SubpixelMask => {
                    // Use luminance of subpixel data as alpha.
                    img.data
                        .chunks(3)
                        .map(|rgb| ((rgb[0] as u16 + rgb[1] as u16 + rgb[2] as u16) / 3) as u8)
                        .collect()
                }
            };

            let w = img.placement.width;
            let h = img.placement.height;

            let (x, y) = self.allocate(w, h)?;
            self.upload_region(queue, x, y, w, h, &alpha_data);

            let s = self.size as f32;
            Some(AtlasEntry {
                uv: [
                    x as f32 / s,
                    y as f32 / s,
                    (x + w) as f32 / s,
                    (y + h) as f32 / s,
                ],
                placement_left: img.placement.left,
                placement_top: img.placement.top,
                width: w,
                height: h,
            })
        });

        self.cache.insert(cache_key, entry);
        entry
    }

    /// Allocate space for a glyph in the atlas using shelf-packing.
    fn allocate(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        // Try to fit in an existing shelf.
        for shelf in &mut self.shelves {
            if h <= shelf.height && shelf.x_cursor + w <= self.size {
                let x = shelf.x_cursor;
                let y = shelf.y;
                shelf.x_cursor += w + 1; // 1px padding
                return Some((x, y));
            }
        }

        // Create a new shelf.
        let shelf_y = self
            .shelves
            .last()
            .map(|s| s.y + s.height + 1) // 1px padding between shelves
            .unwrap_or(0);

        if shelf_y + h > self.size || w > self.size {
            tracing::warn!("Glyph atlas full, cannot allocate {}x{}", w, h);
            return None;
        }

        let x = 0;
        self.shelves.push(Shelf {
            y: shelf_y,
            height: h,
            x_cursor: w + 1,
        });
        Some((x, shelf_y))
    }

    /// Upload glyph alpha data to a region of the atlas texture.
    fn upload_region(&self, queue: &wgpu::Queue, x: u32, y: u32, w: u32, h: u32, data: &[u8]) {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }
}
