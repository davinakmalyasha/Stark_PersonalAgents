"""Composite / compare helper for the Electron overlay review harness.

Usage:
  composite <overlay_png> <backdrop_png> <x> <y> <out_png>
      Paste the (possibly transparent) overlay capture onto the desktop
      backdrop at physical pixel offset (x, y).
  regiondiff <a_png> <b_png> <x> <y> <w> <h>
      Print mean absolute per-pixel difference of the rect between two
      same-size images (used for the no-mirror assertion).
"""
import sys

from PIL import Image, ImageChops, ImageStat


def composite(overlay_path, backdrop_path, x, y, out_path):
    base = Image.open(backdrop_path).convert("RGBA")
    over = Image.open(overlay_path).convert("RGBA")
    base.paste(over, (int(x), int(y)), over)
    base.convert("RGB").save(out_path)
    print(f"composite -> {out_path}")


def regiondiff(a_path, b_path, x, y, w, h):
    a = Image.open(a_path).convert("RGB")
    b = Image.open(b_path).convert("RGB")
    box = (int(x), int(y), int(x) + int(w), int(y) + int(h))
    diff = ImageChops.difference(a.crop(box), b.crop(box))
    mean = ImageStat.Stat(diff).mean
    score = sum(mean) / 3.0
    print(f"regiondiff mean={score:.2f}")
    return score


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "composite":
        composite(*sys.argv[2:7])
    elif cmd == "regiondiff":
        regiondiff(*sys.argv[2:8])
    else:
        raise SystemExit(f"unknown command {cmd}")
