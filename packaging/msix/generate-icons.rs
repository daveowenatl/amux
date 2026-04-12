#!/usr/bin/env -S cargo +nightly -Zscript
//! Generate placeholder MSIX icon assets.
//!
//! Usage: `cargo +nightly -Zscript packaging/msix/generate-icons.rs`
//!    Or: `rustc packaging/msix/generate-icons.rs -o gen-icons && ./gen-icons`
//!
//! Creates minimal dark-background PNG placeholders at the sizes required
//! by AppxManifest.xml. Replace with real branding before release.

fn main() {
    let assets_dir = std::path::PathBuf::from("packaging/msix/Assets");
    std::fs::create_dir_all(&assets_dir).expect("failed to create Assets dir");

    let icons: &[(&str, u32, u32)] = &[
        ("Square44x44Logo.scale-100.png", 44, 44),
        ("Square44x44Logo.scale-200.png", 88, 88),
        (
            "Square44x44Logo.targetsize-44_altform-unplated.png",
            44,
            44,
        ),
        ("Square150x150Logo.scale-100.png", 150, 150),
        ("Square150x150Logo.scale-200.png", 300, 300),
        ("Wide310x150Logo.scale-100.png", 310, 150),
        ("Wide310x150Logo.scale-200.png", 620, 300),
        ("StoreLogo.scale-100.png", 50, 50),
    ];

    for (name, w, h) in icons {
        create_placeholder(&assets_dir, name, *w, *h);
    }
    println!("Done. Replace with real branding before release.");
}

/// Create a dark-grey PNG with "amux" centered in lighter grey.
fn create_placeholder(dir: &std::path::Path, name: &str, w: u32, h: u32) {
    // Simple RGBA buffer: dark background
    let bg = [30u8, 30, 30, 255];
    let fg = [180u8, 180, 180, 255];
    let mut pixels = vec![0u8; (w * h * 4) as usize];

    // Fill background
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.copy_from_slice(&bg);
    }

    // Draw a simple "A" shape as a recognisable placeholder (5x7 grid scaled)
    let glyph: &[&[u8]] = &[
        b" ### ",
        b"#   #",
        b"#   #",
        b"#####",
        b"#   #",
        b"#   #",
        b"#   #",
    ];
    let gw = 5u32;
    let gh = 7u32;
    let scale = (w.min(h) / 12).max(1);
    let ox = (w - gw * scale) / 2;
    let oy = (h - gh * scale) / 2;

    for (gy, row) in glyph.iter().enumerate() {
        for (gx, &ch) in row.iter().enumerate() {
            if ch == b'#' {
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px = ox + gx as u32 * scale + sx;
                        let py = oy + gy as u32 * scale + sy;
                        if px < w && py < h {
                            let idx = ((py * w + px) * 4) as usize;
                            pixels[idx..idx + 4].copy_from_slice(&fg);
                        }
                    }
                }
            }
        }
    }

    // Write PNG manually using miniz_oxide for deflate (no external deps)
    let path = dir.join(name);
    write_png(&path, &pixels, w, h);
    println!("  {name} ({w}x{h})");
}

fn write_png(path: &std::path::Path, rgba: &[u8], w: u32, h: u32) {
    let mut raw = Vec::with_capacity(rgba.len() + h as usize);
    for y in 0..h {
        raw.push(0u8); // filter: None
        let start = (y * w * 4) as usize;
        let end = start + (w * 4) as usize;
        raw.extend_from_slice(&rgba[start..end]);
    }

    // Deflate using flate2-style: we'll use a simple store-only deflate
    let compressed = deflate_store(&raw);

    let mut out = Vec::new();
    // PNG signature
    out.extend_from_slice(&[137, 80, 78, 71, 13, 10, 26, 10]);
    // IHDR
    write_chunk(&mut out, b"IHDR", &{
        let mut d = Vec::new();
        d.extend_from_slice(&w.to_be_bytes());
        d.extend_from_slice(&h.to_be_bytes());
        d.push(8); // bit depth
        d.push(6); // color type: RGBA
        d.push(0); // compression
        d.push(0); // filter
        d.push(0); // interlace
        d
    });
    // IDAT
    write_chunk(&mut out, b"IDAT", &compressed);
    // IEND
    write_chunk(&mut out, b"IEND", &[]);

    std::fs::write(path, &out).expect("failed to write PNG");
}

fn write_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(chunk_type);
    out.extend_from_slice(data);
    let crc = crc32(chunk_type, data);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(chunk_type: &[u8], data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in chunk_type.iter().chain(data.iter()) {
        crc ^= b as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

/// Minimal deflate using only stored blocks (no compression).
/// Produces valid zlib stream that any PNG decoder will accept.
fn deflate_store(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    // Zlib header: CM=8 (deflate), CINFO=7 (32K window), FCHECK so header%31==0
    out.push(0x78);
    out.push(0x01);

    // Split into 65535-byte stored blocks
    let chunks: Vec<&[u8]> = data.chunks(65535).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let is_last = i == chunks.len() - 1;
        out.push(if is_last { 1 } else { 0 }); // BFINAL + BTYPE=00 (stored)
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
    }

    // Adler-32 checksum
    let adler = adler32(data);
    out.extend_from_slice(&adler.to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}
