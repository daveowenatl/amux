//! System font fallback loading and bold font helper.

use crate::*;

/// Cached font family for semibold/bold UI text (sidebar titles, active tabs).
/// Uses a static Arc<str> to avoid allocating on every call.
pub(crate) fn bold_font(size: f32) -> egui::FontId {
    use std::sync::LazyLock;
    static BOLD_FAMILY: LazyLock<egui::FontFamily> =
        LazyLock::new(|| egui::FontFamily::Name("Bold".into()));
    egui::FontId::new(size, BOLD_FAMILY.clone())
}

/// Load system fonts as fallbacks to egui's font families.
/// This provides coverage for braille patterns, geometric shapes, and other symbols
/// that egui's bundled Hack font doesn't include.
///
/// Also registers bundled IBM Plex Sans (Regular + SemiBold) for consistent
/// cross-platform UI text, and a custom "Bold" font family for titles.
pub(crate) fn install_system_font_fallback(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut loaded = Vec::new();

    // --- Bundled IBM Plex Sans for UI chrome ---
    fonts.font_data.insert(
        "plex_sans".to_owned(),
        egui::FontData::from_static(font::SANS_REGULAR).into(),
    );
    fonts.font_data.insert(
        "plex_sans_semibold".to_owned(),
        egui::FontData::from_static(font::SANS_SEMIBOLD).into(),
    );

    // Platform-specific font candidates: (path, name, is_symbol_font, is_proportional)
    // We try to load a monospace font + a symbols font for maximum coverage.
    let candidates: &[(&str, &str, bool, bool)] = if cfg!(target_os = "macos") {
        &[
            // SF Pro (system font) for UI chrome text
            ("/System/Library/Fonts/SFNS.ttf", "sf_pro", false, true),
            // SF Mono: single .ttf with good Unicode coverage
            (
                "/System/Library/Fonts/SFNSMono.ttf",
                "sf_mono",
                false,
                false,
            ),
            // Apple Symbols: broad Unicode symbol coverage
            (
                "/System/Library/Fonts/Apple Symbols.ttf",
                "apple_symbols",
                true,
                false,
            ),
            // Supplemental Andale Mono as another option
            (
                "/System/Library/Fonts/Supplemental/Andale Mono.ttf",
                "andale_mono",
                false,
                false,
            ),
        ]
    } else if cfg!(target_os = "windows") {
        &[
            ("C:\\Windows\\Fonts\\segoeui.ttf", "segoe_ui", false, true),
            ("C:\\Windows\\Fonts\\consola.ttf", "consolas", false, false),
            (
                "C:\\Windows\\Fonts\\segmdl2.ttf",
                "segoe_symbols",
                true,
                false,
            ),
        ]
    } else {
        &[
            (
                "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
                "dejavu_mono",
                false,
                false,
            ),
            (
                "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
                "dejavu_mono",
                false,
                false,
            ),
        ]
    };

    // Bold system font candidates: (path, name)
    // Loaded separately so they can be added to the "Bold" family.
    let bold_candidates: &[(&str, &str)] = if cfg!(target_os = "macos") {
        // macOS provides bold weights / variable font axes for SF Pro, but we
        // intentionally avoid hard-coding those paths or relying on axis selection.
        // Bundled Plex Sans SemiBold is used instead for bold UI text.
        &[]
    } else if cfg!(target_os = "windows") {
        &[("C:\\Windows\\Fonts\\segoeuib.ttf", "segoe_ui_bold")]
    } else {
        &[(
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "dejavu_bold",
        )]
    };

    let mut proportional_fonts = Vec::new();
    let mut mono_fonts = Vec::new();
    let mut bold_fonts = Vec::new();

    for &(path, name, _is_symbol, is_proportional) in candidates {
        if fonts.font_data.contains_key(name) {
            continue;
        }
        match std::fs::read(path) {
            Ok(data) => {
                fonts
                    .font_data
                    .insert(name.to_owned(), egui::FontData::from_owned(data).into());
                loaded.push(name);
                if is_proportional {
                    proportional_fonts.push(name);
                } else {
                    mono_fonts.push(name);
                }
            }
            Err(e) => {
                tracing::debug!("Font fallback not found: {} ({})", path, e);
            }
        }
    }

    for &(path, name) in bold_candidates {
        if fonts.font_data.contains_key(name) {
            continue;
        }
        match std::fs::read(path) {
            Ok(data) => {
                fonts
                    .font_data
                    .insert(name.to_owned(), egui::FontData::from_owned(data).into());
                bold_fonts.push(name);
                loaded.push(name);
            }
            Err(e) => {
                tracing::debug!("Bold font not found: {} ({})", path, e);
            }
        }
    }

    // Proportional fonts: bundled Plex Sans first, then system UI font, then mono
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, "plex_sans".to_owned());
        for name in &proportional_fonts {
            family.insert(1, (*name).to_owned());
        }
        for name in &mono_fonts {
            family.push((*name).to_owned());
        }
    }
    // Monospace fonts: add all as fallbacks
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
        for name in &loaded {
            family.push((*name).to_owned());
        }
    }

    // "Bold" family: bundled Plex Sans SemiBold first, then system bold fonts,
    // then fall back to all proportional + mono fonts for symbol coverage.
    let mut bold_family = vec!["plex_sans_semibold".to_owned()];
    for name in &bold_fonts {
        bold_family.push((*name).to_owned());
    }
    for name in &proportional_fonts {
        bold_family.push((*name).to_owned());
    }
    for name in &mono_fonts {
        bold_family.push((*name).to_owned());
    }
    // Include egui's bundled fonts as final fallback
    if let Some(prop) = fonts.families.get(&egui::FontFamily::Proportional) {
        for name in prop {
            if !bold_family.contains(name) {
                bold_family.push(name.clone());
            }
        }
    }
    fonts
        .families
        .insert(egui::FontFamily::Name("Bold".into()), bold_family);

    if loaded.is_empty() {
        tracing::warn!("No system font fallbacks found; egui may miss symbol/emoji coverage");
    }

    tracing::info!("Loaded font fallbacks: {:?}", loaded);
    ctx.set_fonts(fonts);
}
