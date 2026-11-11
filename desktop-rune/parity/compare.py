#!/usr/bin/env python3
"""Compare a native Rune capture with a React capture at the same size.

    python parity/compare.py --native native.png --react react.png [--out DIR]
                             [--align] [--blur 1] [--max 2.0]

Prints full/right/band means, saves a heatmap, and with `--align` finds the
best ±6 px global offset (a layout shift, not anti-aliasing). `--blur` compares
softened images so font rasterisation differences stop dominating.
"""
import argparse
import json
import pathlib
import sys

import numpy as np
from PIL import Image, ImageFilter


def load(path: pathlib.Path, blur: float) -> np.ndarray:
    im = Image.open(path).convert("RGB")
    if blur > 0:
        im = im.filter(ImageFilter.GaussianBlur(blur))
    return np.asarray(im).astype(np.int16)


def diff(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    return np.abs(a - b).mean(axis=2)


def region_mean(d: np.ndarray, x0: int, y0: int, x1: int, y1: int) -> float:
    return float(d[y0:y1, x0:x1].mean())


def best_shift(a: np.ndarray, b: np.ndarray, radius: int = 6):
    """Shift b by (dx, dy) to line up with a; returns the lowest mean."""
    h, w = a.shape[:2]
    best = (float("inf"), 0, 0)
    for dy in range(-radius, radius + 1):
        for dx in range(-radius, radius + 1):
            x0, x1 = max(0, dx), min(w, w + dx)
            y0, y1 = max(0, dy), min(h, h + dy)
            aa = a[y0:y1, x0:x1]
            bb = b[y0 - dy : y1 - dy, x0 - dx : x1 - dx]
            if aa.size == 0:
                continue
            m = float(np.abs(aa - bb).mean())
            if m < best[0]:
                best = (m, dx, dy)
    return best


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--native", required=True, type=pathlib.Path)
    ap.add_argument("--react", required=True, type=pathlib.Path)
    ap.add_argument("--out", type=pathlib.Path)
    ap.add_argument("--align", action="store_true", help="search ±6 px for a global offset")
    ap.add_argument("--blur", type=float, default=0.0, help="Gaussian radius before diffing")
    ap.add_argument("--max", type=float, default=None, help="fail above this full mean")
    ap.add_argument("--json", type=pathlib.Path)
    args = ap.parse_args()

    a = load(args.native, args.blur)
    b = load(args.react, args.blur)
    if a.shape != b.shape:
        print(f"size mismatch: native {a.shape[1]}x{a.shape[0]}, react {b.shape[1]}x{b.shape[0]}")
        return 2

    d = diff(a, b)
    h, w = d.shape
    out = {
        "size": [w, h],
        "blur": args.blur,
        "full_mean": float(d.mean()),
        "right_mean": region_mean(d, w // 2, 0, w, h),
        "max": float(d.max()),
        "over_12": int((d > 12).sum()),
        "bands": [
            {"y": y, "mean": region_mean(d, 0, y, w, min(h, y + 40)),
             "hot": int((d[y : y + 40] > 12).sum())}
            for y in range(0, h, 40)
        ],
    }
    if args.align:
        native = load(args.native, args.blur)
        react = load(args.react, args.blur)
        mean, dx, dy = best_shift(native, react)
        out["align"] = {"mean": mean, "dx": dx, "dy": dy}

    print(json.dumps(out, indent=2))
    if args.out:
        args.out.mkdir(parents=True, exist_ok=True)
        Image.fromarray(np.clip(d * 4, 0, 255).astype("uint8")).save(args.out / "heatmap.png")
        Image.fromarray(np.clip((d > 12) * 255, 0, 255).astype("uint8")).save(args.out / "mask.png")
    if args.json:
        args.json.write_text(json.dumps(out, indent=2), encoding="utf-8")
    if args.max is not None and out["full_mean"] > args.max:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
