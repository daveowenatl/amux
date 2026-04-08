//! Tab icon management: terminal icon rendering and favicon fetching/caching.

use crate::*;

/// Size of tab icons in logical pixels.
pub(crate) const ICON_SIZE: f32 = 14.0;

/// Paint a filled terminal icon (rounded rect with dark ">_" inside).
pub(crate) fn paint_terminal_icon(
    painter: &egui::Painter,
    top_left: egui::Pos2,
    size: f32,
    color: egui::Color32,
) {
    let rect = egui::Rect::from_min_size(top_left, egui::vec2(size, size));
    let rounding = size * 0.18;

    // Filled rounded rectangle background
    painter.rect_filled(rect, rounding, color);

    // Dark ">_" glyphs inside
    let glyph_color = egui::Color32::from_gray(30);
    let line_width = (size * 0.12).max(1.0);

    // ">" chevron
    let inset = size * 0.22;
    let chevron_left = rect.min.x + inset;
    let chevron_right = rect.min.x + size * 0.48;
    let chevron_top = rect.min.y + size * 0.28;
    let chevron_mid = rect.min.y + size * 0.52;
    let chevron_bot = rect.min.y + size * 0.76;

    painter.line_segment(
        [
            egui::pos2(chevron_left, chevron_top),
            egui::pos2(chevron_right, chevron_mid),
        ],
        egui::Stroke::new(line_width, glyph_color),
    );
    painter.line_segment(
        [
            egui::pos2(chevron_right, chevron_mid),
            egui::pos2(chevron_left, chevron_bot),
        ],
        egui::Stroke::new(line_width, glyph_color),
    );

    // "_" underscore
    let underscore_left = rect.min.x + size * 0.5;
    let underscore_right = rect.max.x - inset;
    let underscore_y = rect.min.y + size * 0.76;

    painter.line_segment(
        [
            egui::pos2(underscore_left, underscore_y),
            egui::pos2(underscore_right, underscore_y),
        ],
        egui::Stroke::new(line_width, glyph_color),
    );
}

/// Paint a split-vertical icon (two side-by-side rectangles with a divider).
pub(crate) fn paint_split_vertical_icon(
    painter: &egui::Painter,
    top_left: egui::Pos2,
    size: f32,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new((size * 0.1).max(1.0), color);
    let rounding = size * 0.12;
    let rect = egui::Rect::from_min_size(top_left, egui::vec2(size, size));

    // Outer rounded rect
    painter.rect_stroke(rect, rounding, stroke, egui::StrokeKind::Inside);
    // Vertical divider in the middle
    let mid_x = rect.center().x;
    painter.line_segment(
        [
            egui::pos2(mid_x, rect.min.y + 2.0),
            egui::pos2(mid_x, rect.max.y - 2.0),
        ],
        stroke,
    );
}

/// Paint a split-horizontal icon (two stacked rectangles with a divider).
pub(crate) fn paint_split_horizontal_icon(
    painter: &egui::Painter,
    top_left: egui::Pos2,
    size: f32,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new((size * 0.1).max(1.0), color);
    let rounding = size * 0.12;
    let rect = egui::Rect::from_min_size(top_left, egui::vec2(size, size));

    // Outer rounded rect
    painter.rect_stroke(rect, rounding, stroke, egui::StrokeKind::Inside);
    // Horizontal divider in the middle
    let mid_y = rect.center().y;
    painter.line_segment(
        [
            egui::pos2(rect.min.x + 2.0, mid_y),
            egui::pos2(rect.max.x - 2.0, mid_y),
        ],
        stroke,
    );
}

/// Paint a globe icon (circle with horizontal and vertical arcs).
pub(crate) fn paint_globe_icon(
    painter: &egui::Painter,
    top_left: egui::Pos2,
    size: f32,
    color: egui::Color32,
) {
    let stroke = egui::Stroke::new((size * 0.1).max(1.0), color);
    let center = egui::pos2(top_left.x + size / 2.0, top_left.y + size / 2.0);
    let r = size / 2.0 - 0.5;

    // Outer circle
    painter.circle_stroke(center, r, stroke);
    // Horizontal line through middle
    painter.line_segment(
        [
            egui::pos2(center.x - r, center.y),
            egui::pos2(center.x + r, center.y),
        ],
        stroke,
    );
    // Vertical ellipse (two arcs approximated via line segments)
    let inner_r = r * 0.5;
    // Left arc approximation
    let steps = 8;
    for i in 0..steps {
        let t0 = std::f32::consts::PI / 2.0 + (i as f32 / steps as f32) * std::f32::consts::PI;
        let t1 =
            std::f32::consts::PI / 2.0 + ((i + 1) as f32 / steps as f32) * std::f32::consts::PI;
        painter.line_segment(
            [
                egui::pos2(center.x + inner_r * t0.cos(), center.y + r * t0.sin()),
                egui::pos2(center.x + inner_r * t1.cos(), center.y + r * t1.sin()),
            ],
            stroke,
        );
    }
    // Right arc
    for i in 0..steps {
        let t0 = -std::f32::consts::PI / 2.0 + (i as f32 / steps as f32) * std::f32::consts::PI;
        let t1 =
            -std::f32::consts::PI / 2.0 + ((i + 1) as f32 / steps as f32) * std::f32::consts::PI;
        painter.line_segment(
            [
                egui::pos2(center.x + inner_r * t0.cos(), center.y + r * t0.sin()),
                egui::pos2(center.x + inner_r * t1.cos(), center.y + r * t1.sin()),
            ],
            stroke,
        );
    }
}

impl AmuxApp {
    /// Get a favicon texture for a browser pane, initiating a JS fetch if needed.
    /// Returns None if the favicon hasn't been fetched/decoded yet.
    pub(crate) fn get_favicon(
        &mut self,
        _ctx: &egui::Context,
        favicon_url: &str,
        browser_pane_id: PaneId,
    ) -> Option<egui::TextureId> {
        // Already cached?
        if let Some(tex) = self.favicon_cache.get(favicon_url) {
            return Some(tex.id());
        }

        // Start a fetch via the webview's JS (avoids HTTPS/CORS issues)
        if !self.favicon_pending.contains(favicon_url) {
            self.favicon_pending.insert(favicon_url.to_string());
            if let Some(PaneEntry::Browser(browser)) = self.panes.get(&browser_pane_id) {
                // Serialize the URL as a JSON string to safely embed it in JS
                // (handles backslashes, newlines, quotes, and all other escapes).
                let url_json =
                    serde_json::to_string(favicon_url).unwrap_or_else(|_| "\"\"".to_string());
                browser.evaluate_script(&format!(
                    r#"(function(){{
                        var u={url_json};
                        fetch(u).then(function(r) {{
                            return r.blob();
                        }}).then(function(blob) {{
                            var reader = new FileReader();
                            reader.onloadend = function() {{
                                var b64 = reader.result.split(',')[1];
                                if (b64) {{
                                    window.ipc.postMessage(JSON.stringify({{
                                        type:'favicon_data',
                                        url:u,
                                        data:b64
                                    }}));
                                }}
                            }};
                            reader.readAsDataURL(blob);
                        }}).catch(function(e) {{}});
                    }})()"#
                ));
            }
        }

        None
    }

    /// Process favicon data received from browser panes via IPC.
    pub(crate) fn process_favicon_data(&mut self, ctx: &egui::Context) {
        let mut favicon_data: Vec<(String, Vec<u8>)> = Vec::new();
        for entry in self.panes.values() {
            if let PaneEntry::Browser(browser) = entry {
                favicon_data.extend(browser.drain_favicon_data());
            }
        }
        for (url, data) in favicon_data {
            self.favicon_pending.remove(&url);
            if let Some(image) = decode_favicon(&data) {
                let tex = ctx.load_texture(
                    format!("favicon_{url}"),
                    image,
                    egui::TextureOptions::LINEAR,
                );
                self.favicon_cache.insert(url, tex);

                // Cap cache at 256 entries
                while self.favicon_cache.len() > 256 {
                    if let Some(key) = self.favicon_cache.keys().next().cloned() {
                        self.favicon_cache.remove(&key);
                    }
                }
            }
        }
    }
}

/// Decode raw image bytes into an egui ColorImage, resized to icon dimensions.
fn decode_favicon(data: &[u8]) -> Option<egui::ColorImage> {
    let img = image::load_from_memory(data).ok()?;
    let img = img.resize_exact(32, 32, image::imageops::FilterType::Lanczos3);
    let rgba = img.to_rgba8();
    let pixels: Vec<egui::Color32> = rgba
        .pixels()
        .map(|p| egui::Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]))
        .collect();
    Some(egui::ColorImage {
        size: [32, 32],
        pixels,
    })
}
