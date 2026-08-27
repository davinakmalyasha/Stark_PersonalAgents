"""Generate PWA icons for the mobile/remote feature (run once via venv312)."""
import os
from PIL import Image, ImageDraw

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "frontend", "public", "icons")
os.makedirs(OUT, exist_ok=True)

BG = (12, 12, 14, 255)        # #0c0c0e — app chrome color
BOLT = (56, 189, 248, 255)    # #38bdf8 — sky-400 accent
CORNER = 22                    # rounded-corner radius fraction %


def bolt_polygon(size: int, scale: float):
    """Lightning-bolt polygon points normalized to a size x scale box."""
    w = size * scale
    h = size * scale
    ox = (size - w) / 2
    oy = (size - h) / 2
    pts = [
        (0.62, 0.00), (0.10, 0.55), (0.42, 0.55),
        (0.34, 1.00), (0.90, 0.40), (0.55, 0.40),
    ]
    return [(ox + px * w, oy + py * h) for px, py in pts]


def make_icon(px: int, maskable: bool = False) -> Image.Image:
    img = Image.new("RGBA", (px, px), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    r = 0 if maskable else px * CORNER // 100
    d.rounded_rectangle([0, 0, px - 1, px - 1], radius=r, fill=BG)
    scale = 0.58 if maskable else 0.68  # maskable keeps content inside safe zone
    d.polygon(bolt_polygon(px, scale), fill=BOLT)
    return img


for name, size, maskable in [
    ("pwa-192.png", 192, False),
    ("pwa-512.png", 512, False),
    ("pwa-maskable-192.png", 192, True),
    ("pwa-maskable-512.png", 512, True),
]:
    make_icon(size, maskable).save(os.path.join(OUT, name))
    print(f"wrote {name} ({size}px)")

# Also fix the Electron builder's missing icon reference.
make_icon(256).convert("RGB").save(os.path.join(OUT, "electron-icon-256.png"))
print("wrote electron-icon-256.png")
