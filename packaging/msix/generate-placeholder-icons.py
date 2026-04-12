#!/usr/bin/env python3
"""Generate placeholder MSIX icon assets.

Creates minimal PNG files at the required sizes for MSIX packaging.
These are developer placeholders — replace with real branding before
shipping to users.

Requires: pip install Pillow
"""

from pathlib import Path

try:
    from PIL import Image, ImageDraw, ImageFont
except ImportError:
    print("Pillow not installed. Install with: pip install Pillow")
    print("Or create PNG files manually at these sizes in Assets/:")
    print("  Square44x44Logo.scale-100.png    (44x44)")
    print("  Square44x44Logo.scale-200.png    (88x88)")
    print("  Square44x44Logo.targetsize-44_altform-unplated.png (44x44)")
    print("  Square150x150Logo.scale-100.png  (150x150)")
    print("  Square150x150Logo.scale-200.png  (300x300)")
    print("  Wide310x150Logo.scale-100.png    (310x150)")
    print("  Wide310x150Logo.scale-200.png    (620x300)")
    print("  StoreLogo.scale-100.png          (50x50)")
    raise SystemExit(1)

ASSETS_DIR = Path(__file__).parent / "Assets"
ASSETS_DIR.mkdir(exist_ok=True)

BG_COLOR = (30, 30, 30)       # dark background
TEXT_COLOR = (200, 200, 200)   # light text

ICONS = [
    ("Square44x44Logo.scale-100.png", 44, 44),
    ("Square44x44Logo.scale-200.png", 88, 88),
    ("Square44x44Logo.targetsize-44_altform-unplated.png", 44, 44),
    ("Square150x150Logo.scale-100.png", 150, 150),
    ("Square150x150Logo.scale-200.png", 300, 300),
    ("Wide310x150Logo.scale-100.png", 310, 150),
    ("Wide310x150Logo.scale-200.png", 620, 300),
    ("StoreLogo.scale-100.png", 50, 50),
]


def create_icon(name: str, width: int, height: int) -> None:
    img = Image.new("RGBA", (width, height), BG_COLOR + (255,))
    draw = ImageDraw.Draw(img)

    label = "amux"
    font_size = min(width, height) // 3
    try:
        font = ImageFont.truetype("consola.ttf", font_size)
    except OSError:
        font = ImageFont.load_default()

    bbox = draw.textbbox((0, 0), label, font=font)
    tw, th = bbox[2] - bbox[0], bbox[3] - bbox[1]
    x = (width - tw) // 2
    y = (height - th) // 2
    draw.text((x, y), label, fill=TEXT_COLOR, font=font)

    path = ASSETS_DIR / name
    img.save(path, "PNG")
    print(f"  {name} ({width}x{height})")


if __name__ == "__main__":
    print(f"Generating placeholder icons in {ASSETS_DIR}/")
    for name, w, h in ICONS:
        create_icon(name, w, h)
    print("Done. Replace these with real branding before release.")
