#!/usr/bin/env python3
"""Generate amux icons: main app icon, MSIX tiles, Windows .ico.

Renders a lowercase 'a' in JetBrains Mono Bold (Monokai orange) on a
rounded-rect dark background (Monokai bg) — matching amux's in-app theme.

Requires: pip install Pillow

Output:
  Assets/icon-1024.png           — main window / dock / taskbar icon
  Assets/Square44x44Logo*.png    — MSIX small tile (4 variants)
  Assets/Square150x150Logo*.png  — MSIX medium tile (2 variants)
  Assets/Wide310x150Logo*.png    — MSIX wide tile (2 variants)
  Assets/StoreLogo.scale-100.png — MSIX store icon
  Assets/amux.ico                — Windows .exe multi-size icon
"""

from __future__ import annotations
from pathlib import Path
from typing import Optional

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:
    print("Pillow not installed. Install with: pip install Pillow")
    raise SystemExit(1)

HERE = Path(__file__).parent
ASSETS_DIR = HERE / "Assets"
ASSETS_DIR.mkdir(exist_ok=True)

# Bundled font (same file used for terminal rendering).
FONT_PATH = HERE.parent.parent / "crates/amux-term/fonts/JetBrainsMono-Bold.ttf"

# Colors — matches Monokai palette + amux theme.
BG = (37, 40, 48, 255)        # #252830 — Monokai background
FG = (253, 151, 31, 255)      # #fd971f — Monokai orange
TRANSPARENT = (0, 0, 0, 0)

# Font size as a fraction of icon height. 0.75 puts the 'a' much bigger
# than the previous ~0.45. Rounded-rect corner radius is fraction of size.
GLYPH_FRACTION = 0.75
CORNER_FRACTION = 0.18


def render_icon(size: int, *, rounded: bool, wide_width: Optional[int] = None) -> Image.Image:
    """Render a square or wide icon with a centered 'a' glyph.

    If `wide_width` is given, the icon is `wide_width x size` and the glyph
    is centered horizontally within that wider canvas. Otherwise square.
    """
    w = wide_width if wide_width else size
    h = size

    # Start transparent so rounded corners are visible on whatever OS chrome
    # composites underneath. MSIX tiles that need opaque bg are handled by
    # the compositor; our rounded-rect fill provides the visible surface.
    img = Image.new("RGBA", (w, h), TRANSPARENT)
    draw = ImageDraw.Draw(img)

    # Rounded dark background (or square if size is tiny).
    radius = int(size * CORNER_FRACTION)
    if size < 32:
        # At 16x16 / 24x24, rounded corners look like stair-step garbage.
        # Use a plain rectangle instead.
        draw.rectangle([(0, 0), (w - 1, h - 1)], fill=BG)
    else:
        draw.rounded_rectangle([(0, 0), (w - 1, h - 1)], radius=radius, fill=BG)

    # Font at GLYPH_FRACTION of icon height.
    font_size = max(int(size * GLYPH_FRACTION), 8)
    font = ImageFont.truetype(str(FONT_PATH), font_size)

    # Measure the glyph ink bounds (actual pixels drawn) rather than the
    # font's advance box, so we can center the visible glyph. textbbox
    # returns the *ink* bounding box when anchor="lt".
    label = "a"
    bbox = draw.textbbox((0, 0), label, font=font, anchor="lt")
    tw = bbox[2] - bbox[0]
    th = bbox[3] - bbox[1]

    # Nudge up slightly: lowercase 'a' has its optical center above geometric
    # center because it sits on the baseline with no descender.
    x = (w - tw) // 2 - bbox[0]
    y = (h - th) // 2 - bbox[1]
    draw.text((x, y), label, fill=FG, font=font, anchor="lt")

    return img


def save_png(img: Image.Image, name: str) -> None:
    path = ASSETS_DIR / name
    img.save(path, "PNG")
    print(f"  {name} ({img.width}x{img.height})")


MSIX_SQUARE_SIZES = [
    ("Square44x44Logo.scale-100.png", 44),
    ("Square44x44Logo.scale-200.png", 88),
    ("Square44x44Logo.targetsize-44_altform-unplated.png", 44),
    ("Square150x150Logo.scale-100.png", 150),
    ("Square150x150Logo.scale-200.png", 300),
    ("StoreLogo.scale-100.png", 50),
]

MSIX_WIDE_SIZES = [
    # (name, width, height)
    ("Wide310x150Logo.scale-100.png", 310, 150),
    ("Wide310x150Logo.scale-200.png", 620, 300),
]

# Windows .ico: include all standard sizes so Explorer/Taskbar picks the
# right one for each DPI. Icons <32px render as plain squares (no rounded
# corners) since the rounding becomes stair-steppy at low res.
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]


def main() -> None:
    if not FONT_PATH.exists():
        raise SystemExit(f"Font not found: {FONT_PATH}")

    print(f"Generating icons in {ASSETS_DIR}/")
    print(f"  font: {FONT_PATH.name}")
    print(f"  glyph fraction: {GLYPH_FRACTION}")
    print(f"  bg: {BG[:3]}  fg: {FG[:3]}")
    print()

    # Main 1024 app icon (used for window, dock, taskbar via load_app_icon).
    save_png(render_icon(1024, rounded=True), "icon-1024.png")

    # MSIX square tiles.
    for name, size in MSIX_SQUARE_SIZES:
        save_png(render_icon(size, rounded=True), name)

    # MSIX wide tiles.
    for name, width, height in MSIX_WIDE_SIZES:
        save_png(render_icon(height, rounded=True, wide_width=width), name)

    # Windows .ico — multi-size.
    ico_path = ASSETS_DIR / "amux.ico"
    ico_images = [render_icon(s, rounded=True) for s in ICO_SIZES]
    ico_images[0].save(
        ico_path,
        format="ICO",
        sizes=[(s, s) for s in ICO_SIZES],
        append_images=ico_images[1:],
    )
    print(f"  amux.ico ({'/'.join(str(s) for s in ICO_SIZES)})")

    print("\nDone.")


if __name__ == "__main__":
    main()
