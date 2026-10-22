"""assets/dog.png -> assets/collar.png: where the collar is in every frame, and what is under it.

Run from the repo root with the Sprite Forge venv (or `make atlas`, after pack_atlas.py).
The collar is found per cell, around the collar point `NECK` (dog_atlas.rs): the red band (the
connected red pixels there, so the tongue and the ears are left out), the gold tag hanging from
it, and a margin around them (their outline). In `assets/collar.png` the band has alpha 255, the
tag 192, the margin 128 and the rest of the dog 100; the colour is the dog without a collar,
the same frame drawn without one (art/px/<anim>/plain, via pack_atlas.py).
"""

import colorsys
import re

import numpy as np
from PIL import Image
from scipy import ndimage

src = open("assets/dog_atlas.rs").read()
CW, CH = map(int, re.search(r"CELL: \(u32, u32\) = \((\d+), (\d+)\)", src).groups())
AX, AY = map(int, re.search(r"ANCHOR: \(u32, u32\) = \((\d+), (\d+)\)", src).groups())
FRAMES = [int(v) for v in re.search(r"FRAMES: \[usize; \d+\] = \[([^\]]*)\]", src).group(1).split(",")]
NECK = [tuple(map(int, p)) for p in re.findall(r"\((-?\d+), (-?\d+)\)", src.split("pub const NECK")[1])]

atlas = np.asarray(Image.open("assets/dog.png").convert("RGBA")).astype(int)
H, W = atlas.shape[:2]


def hls(px):
    r, g, b = px[..., 0] / 255, px[..., 1] / 255, px[..., 2] / 255
    mx, mn = np.maximum(np.maximum(r, g), b), np.minimum(np.minimum(r, g), b)
    l = (mx + mn) / 2
    d = mx - mn
    s = np.where(d == 0, 0, d / np.maximum(1e-6, 1 - np.abs(2 * l - 1)))
    h = np.zeros_like(l)
    m = d > 0
    rr = (mx == r) & m
    gg = (mx == g) & m & ~rr
    bb = m & ~rr & ~gg
    h[rr] = (60 * ((g - b)[rr] / d[rr])) % 360
    h[gg] = 60 * ((b - r)[gg] / d[gg] + 2)
    h[bb] = 60 * ((r - g)[bb] / d[bb] + 4)
    return h, s, l


def collar_cell(c, neck):
    a = c[..., 3] > 0
    h, s, l = hls(c)
    red = a & ((h < 12) | (h > 345)) & (s > 0.5) & (l >= 0.08) & (l < 0.6)
    # The tongue uses the same reds and can touch the collar: leave out red near its pink.
    pink = a & ((h < 20) | (h > 340)) & (s > 0.4) & (l >= 0.6) & (l < 0.88)
    red &= ~ndimage.binary_dilation(pink, iterations=4)
    nx, ny = AX + neck[0], AY + neck[1]
    near = np.zeros_like(a)
    near[max(0, ny - 22) : ny + 22, max(0, nx - 28) : nx + 28] = True
    lab, n = ndimage.label(red & near, structure=np.ones((3, 3)))
    if n == 0:
        return np.zeros_like(a), np.zeros_like(a), np.zeros_like(a)
    sizes = ndimage.sum(np.ones_like(lab), lab, range(1, n + 1))
    # The band: the biggest red piece, and smaller pieces right next to it (the tag splits
    # it); the ears' insides are further away.
    main = lab == 1 + int(np.argmax(sizes))
    close = ndimage.binary_dilation(main, iterations=6)
    keep = [i + 1 for i, sz in enumerate(sizes) if sz >= 3 and (close & (lab == i + 1)).any()]
    band = np.isin(lab, keep)
    # The tag: lemon yellow (its other colours are the golden fur's) hanging just below the
    # band; its box, outline included.
    lemon = a & (h > 47) & (h < 64) & (s > 0.8) & (l > 0.3) & (l < 0.75)
    by = np.nonzero(band.any(1))[0]
    bx = np.nonzero(band.any(0))[0]
    zone = np.zeros_like(a)
    zone[by.min() - 2 : by.max() + 14, bx.min() - 2 : bx.max() + 3] = True
    tag = np.zeros_like(a)
    gl, gn = ndimage.label(lemon & zone, structure=np.ones((3, 3)))
    for i in range(1, gn + 1):
        seed = gl == i
        ys, xs = np.nonzero(seed)
        if len(ys) > 30:
            continue
        cy, cx = int(ys.mean()), int(xs.mean())
        box = np.zeros_like(a)
        box[max(0, cy - 5) : cy + 6, max(0, cx - 5) : cx + 6] = True
        # Inside the tag's outline: what the lemon reaches without crossing dark pixels.
        inside = box & a & (l >= 0.2)
        pl, _ = ndimage.label(inside)
        ids = np.unique(pl[seed & inside])
        tag |= np.isin(pl, ids[ids > 0])
    tag &= ~band
    # Under the collar (filled with fur when it is off): the band, the tag and a two-pixel
    # margin around them (their outline). The dog's own edge stays.
    edge = ndimage.binary_dilation(~a, structure=np.ones((3, 3)))
    region = (ndimage.binary_dilation(band | tag, iterations=2) & a & ~edge) | tag
    return band, tag, region & ~band & ~tag


band_all = np.zeros((H, W), bool)
tag_all = np.zeros((H, W), bool)
rest_all = np.zeros((H, W), bool)
fill = atlas.copy()
cell = 0
for row, n in enumerate(FRAMES):
    for col in range(n):
        y0, x0 = row * CH, col * CW
        c = atlas[y0 : y0 + CH, x0 : x0 + CW]
        band, tag, rest = collar_cell(c, NECK[cell])
        band_all[y0 : y0 + CH, x0 : x0 + CW] = band
        tag_all[y0 : y0 + CH, x0 : x0 + CW] = tag
        rest_all[y0 : y0 + CH, x0 : x0 + CW] = rest
        cell += 1

# What fills the spot without a collar: the same frame drawn by the image model without its
# collar (target/dog-plain.png, written by pack_atlas.py). The mask covers the collar and its
# outline, and grows to wherever the collar-less drawing differs nearby (the chest fur it drew
# instead), so no pixel of the collar is left and the fur is one drawing.
plain = np.asarray(Image.open("target/dog-plain.png").convert("RGBA")).astype(int)
hole = band_all | tag_all | rest_all
alpha = atlas[..., 3] > 0
differs = alpha & (plain[..., 3] > 0) & (np.abs(plain[..., :3] - atlas[..., :3]).sum(-1) > 60)
near = ndimage.binary_dilation(hole, iterations=4)
grow = near & differs & ~hole
# Red the collar-less drawing has not got, next to the collar (the band's end by the tongue,
# which the band finder leaves out): the collar too.
Hh, S, L = hls(atlas)
red = alpha & ((Hh < 12) | (Hh > 345)) & (S > 0.5) & (L < 0.6) & differs
grow |= ndimage.binary_dilation(hole, iterations=8) & red & ~hole
grow = ndimage.binary_dilation(grow, structure=np.ones((3, 3))) & differs & ~hole | grow
rest_all |= grow
hole |= grow
out = np.where((plain[..., 3:] > 0) & hole[..., None], plain, atlas)
# The model redrew more than the collar (the chest fur around it): without a collar the whole
# frame is its drawing, so there is no seam between the two drawings. Those pixels are 100: the
# fur as usual while a collar is worn.
whole = alpha & (plain[..., 3] > 0) & ~hole
out[whole] = plain[whole]
res = np.zeros_like(atlas)
res[..., :3] = out[..., :3]
res[..., 3] = np.where(band_all, 255, np.where(tag_all, 192, np.where(rest_all, 128, np.where(whole, 100, 0))))
res[~(hole | whole)] = 0
Image.fromarray(res.astype(np.uint8)).save("assets/collar.png", optimize=True)
print(f"collar: {band_all.sum()} band, {tag_all.sum()} tag, {rest_all.sum()} outline pixels in {cell} cells")
