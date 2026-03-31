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
    /// Whether this glyph is a color glyph (emoji) vs monochrome.
    pub is_color: bool,
}

/// Shelf in the shelf-packing algorithm.
struct Shelf {
    y: u32,
    height: u32,
    x_cursor: u32,
}

/// A single atlas page (texture + shelf packer).
struct AtlasPage {
    texture: wgpu::Texture,
    texture_view: wgpu::TextureView,
    size: u32,
    shelves: Vec<Shelf>,
    bytes_per_pixel: u32,
}

impl AtlasPage {
    fn new(
        device: &wgpu::Device,
        size: u32,
        format: wgpu::TextureFormat,
        label: &str,
        bytes_per_pixel: u32,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            texture,
            texture_view,
            size,
            shelves: Vec::new(),
            bytes_per_pixel,
        }
    }

    fn allocate(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        // Try to fit in an existing shelf.
        for shelf in &mut self.shelves {
            if h <= shelf.height && shelf.x_cursor + w <= self.size {
                let x = shelf.x_cursor;
                let y = shelf.y;
                shelf.x_cursor += w + 1;
                return Some((x, y));
            }
        }

        // Create a new shelf.
        let shelf_y = self.shelves.last().map(|s| s.y + s.height + 1).unwrap_or(0);

        if shelf_y + h > self.size || w > self.size {
            tracing::warn!("Atlas page full, cannot allocate {}x{}", w, h);
            return None;
        }

        self.shelves.push(Shelf {
            y: shelf_y,
            height: h,
            x_cursor: w + 1,
        });
        Some((0, shelf_y))
    }

    fn upload_region(&self, queue: &wgpu::Queue, x: u32, y: u32, w: u32, h: u32, data: &[u8]) {
        let unpadded_bytes_per_row = w * self.bytes_per_pixel;
        let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(alignment) * alignment;

        let copy_info = wgpu::TexelCopyTextureInfo {
            texture: &self.texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        };
        let extent = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };

        if padded_bytes_per_row == unpadded_bytes_per_row || h <= 1 {
            // Data is already aligned or single row — upload directly.
            queue.write_texture(
                copy_info,
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(h),
                },
                extent,
            );
        } else {
            // Pad each row to satisfy wgpu alignment requirements.
            let mut padded = vec![0u8; (padded_bytes_per_row * h) as usize];
            let row_size = unpadded_bytes_per_row as usize;
            let padded_row_size = padded_bytes_per_row as usize;
            for row in 0..h as usize {
                padded[row * padded_row_size..row * padded_row_size + row_size]
                    .copy_from_slice(&data[row * row_size..row * row_size + row_size]);
            }
            queue.write_texture(
                copy_info,
                &padded,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(h),
                },
                extent,
            );
        }
    }
}

/// GPU glyph atlas using shelf-packing with dual textures.
///
/// Monochrome glyphs use an R8Unorm texture (alpha-only).
/// Color glyphs (emoji) use an Rgba8UnormSrgb texture.
pub struct GlyphAtlas {
    mono: AtlasPage,
    color: AtlasPage,
    pub sampler: wgpu::Sampler,
    cache: HashMap<CacheKey, Option<AtlasEntry>>,
}

impl GlyphAtlas {
    /// Create a new glyph atlas with the given texture size.
    pub fn new(device: &wgpu::Device, size: u32) -> Self {
        let mono = AtlasPage::new(
            device,
            size,
            wgpu::TextureFormat::R8Unorm,
            "glyph_atlas_mono",
            1,
        );
        let color = AtlasPage::new(
            device,
            size,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            "glyph_atlas_color",
            4,
        );

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("glyph_atlas_sampler"),
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Self {
            mono,
            color,
            sampler,
            cache: HashMap::new(),
        }
    }

    /// Get the monochrome atlas texture view.
    pub fn mono_texture_view(&self) -> &wgpu::TextureView {
        &self.mono.texture_view
    }

    /// Get the color atlas texture view.
    pub fn color_texture_view(&self) -> &wgpu::TextureView {
        &self.color.texture_view
    }

    /// Look up or rasterize a glyph, returning `(entry, newly_inserted)`.
    ///
    /// Returns `None` for glyphs that can't be rasterized (e.g., spaces, missing glyphs).
    /// `newly_inserted` is true only when a new glyph was uploaded to the atlas texture.
    pub fn get_or_insert(
        &mut self,
        queue: &wgpu::Queue,
        font_system: &mut FontSystem,
        swash_cache: &mut SwashCache,
        cache_key: CacheKey,
    ) -> (Option<AtlasEntry>, bool) {
        if let Some(entry) = self.cache.get(&cache_key) {
            return (*entry, false);
        }

        let image: Option<SwashImage> = swash_cache.get_image_uncached(font_system, cache_key);

        let entry = image.and_then(|img| {
            if img.placement.width == 0 || img.placement.height == 0 {
                return None;
            }

            let w = img.placement.width;
            let h = img.placement.height;

            let is_color = matches!(img.content, SwashContent::Color);

            if is_color {
                // Color glyph: store RGBA data in the color atlas.
                let (x, y) = self.color.allocate(w, h)?;
                self.color.upload_region(queue, x, y, w, h, &img.data);
                let s = self.color.size as f32;
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
                    is_color: true,
                })
            } else {
                // Monochrome glyph: store alpha data in the mono atlas.
                let alpha_data = match img.content {
                    SwashContent::Mask => img.data,
                    SwashContent::SubpixelMask => img
                        .data
                        .chunks(3)
                        .map(|rgb| ((rgb[0] as u16 + rgb[1] as u16 + rgb[2] as u16) / 3) as u8)
                        .collect(),
                    SwashContent::Color => unreachable!(),
                };

                let (x, y) = self.mono.allocate(w, h)?;
                self.mono.upload_region(queue, x, y, w, h, &alpha_data);
                let s = self.mono.size as f32;
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
                    is_color: false,
                })
            }
        });

        let newly_inserted = entry.is_some();
        self.cache.insert(cache_key, entry);
        (entry, newly_inserted)
    }

    /// Insert a raw monochrome (R8) tile into the atlas.
    /// Used for custom decoration sprites (curly underline, dotted underline).
    /// Returns the atlas entry if allocation succeeds, or None if the atlas is full.
    pub fn insert_raw_mono(
        &mut self,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> Option<AtlasEntry> {
        let (x, y) = self.mono.allocate(width, height)?;
        self.mono.upload_region(queue, x, y, width, height, data);
        let s = self.mono.size as f32;
        Some(AtlasEntry {
            uv: [
                x as f32 / s,
                y as f32 / s,
                (x + width) as f32 / s,
                (y + height) as f32 / s,
            ],
            placement_left: 0,
            placement_top: 0,
            width,
            height,
            is_color: false,
        })
    }
}
