#!/usr/bin/env python3
"""Render the ainxt mark as crisp monochrome BRAILLE terminal art.

Matches the upstream logo technique (U+2800 braille), which packs a 2x4 dot
grid per character cell — 2x the horizontal and 4x the vertical resolution of
half-blocks, so small marks stay legible. The TUI recolors the art with a
single gray->white sheen, so only the silhouette matters: we isolate the bright
mark from the dark background with a luminance threshold, trim to its bounding
box, resize to the target dot grid, and pack dots into braille cells.
"""
from PIL import Image
import numpy as np

SRC = "assets/brand/ainxt-logo.png"

# Braille dot bit for (col_in_cell, row_in_cell). Standard U+28xx layout.
DOT_BITS = {
    (0, 0): 0x01, (0, 1): 0x02, (0, 2): 0x04, (0, 3): 0x40,
    (1, 0): 0x08, (1, 1): 0x10, (1, 2): 0x20, (1, 3): 0x80,
}


def load_mask(threshold=95):
    im = Image.open(SRC).convert("RGB")
    arr = np.asarray(im).astype(np.float32)
    lum = 0.2126 * arr[:, :, 0] + 0.7152 * arr[:, :, 1] + 0.0722 * arr[:, :, 2]
    mask = lum > threshold
    ys, xs = np.where(mask)
    y0, y1, x0, x1 = ys.min(), ys.max(), xs.min(), xs.max()
    return mask[y0:y1 + 1, x0:x1 + 1]


def resize_mask(mask, w_px, h_px):
    img = Image.fromarray((mask * 255).astype(np.uint8))
    img = img.resize((w_px, h_px), Image.LANCZOS)
    a = np.asarray(img).astype(np.float32) / 255.0
    return a > 0.5


def to_braille(mask, cells_w, cells_h):
    w_px = cells_w * 2
    h_px = cells_h * 4
    m = resize_mask(mask, w_px, h_px)
    lines = []
    for cy in range(cells_h):
        line = []
        for cx in range(cells_w):
            bits = 0
            for dx in range(2):
                for dy in range(4):
                    if m[cy * 4 + dy, cx * 2 + dx]:
                        bits |= DOT_BITS[(dx, dy)]
            line.append(chr(0x2800 + bits))
        # Trim trailing blank (U+2800) cells.
        while line and line[-1] == chr(0x2800):
            line.pop()
        lines.append("".join(line))
    while lines and not lines[0].strip("\u2800 "):
        lines.pop(0)
    while lines and not lines[-1].strip("\u2800 "):
        lines.pop()
    return "\n".join(lines) + "\n"


def main():
    mask = load_mask(threshold=95)
    h, w = mask.shape
    aspect = w / h

    # On-screen: a braille cell is ~1 wide x 2 tall (like any char cell), and it
    # holds 2 dots wide x 4 dots tall. So a dot is ~1:1 square. To preserve the
    # mark aspect (w:h) with cells_h rows: dots_h = 4*cells_h, dots_w should be
    # 4*cells_h*aspect, i.e. cells_w = round(2*cells_h*aspect).
    for name, cells_h in [("logo07", 7), ("logo05", 5)]:
        cells_w = max(1, round(2 * cells_h * aspect))
        art = to_braille(mask, cells_w, cells_h)
        path = f"crates/codegen/ainxt-pager/assets/logo/{name}.txt"
        with open(path, "w") as f:
            f.write(art)
        n = len([l for l in art.splitlines() if l.strip("\u2800 ")])
        width = max((len(l) for l in art.splitlines()), default=0)
        print(f"{name}: {n} lines x {width} cols -> {path}")
        print(art)


if __name__ == "__main__":
    main()
