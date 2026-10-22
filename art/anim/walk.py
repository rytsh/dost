"""Walk without boiling: the upper body (head, back, tail) of frame 0 on every frame, moved by
that frame's bob; the legs below LEG_Y from the frame itself. Writes art/anim/fit/walk-<i>.png.
Run: PY art/anim/walk.py [leg_y]"""
import sys
import numpy as np
from PIL import Image
from scipy import ndimage
sys.path.insert(0, "art/anim")
import edit

a = edit.atlas()
fr = [edit.cell(a, "walk", i) for i in range(8)]
LEG = int(sys.argv[1]) if len(sys.argv) > 1 else edit.AY - 40  # cell row where the legs start


def shift(img, dx, dy):
    out = np.zeros_like(img)
    h, w = img.shape[:2]
    ys, yd = (slice(0, h - dy), slice(dy, h)) if dy >= 0 else (slice(-dy, h), slice(0, h + dy))
    xs, xd = (slice(0, w - dx), slice(dx, w)) if dx >= 0 else (slice(-dx, w), slice(0, w + dx))
    out[yd, xd] = img[ys, xs]
    return out


top0 = fr[0].copy()
top0[LEG:] = 0
m0 = top0[..., 3] > 0
# The same upper body without the collar (art/anim/fit/nocollar-walk.png, from walk's frame 0).
plain0 = np.asarray(Image.open("art/anim/fit/nocollar-walk.png").convert("RGBA")).astype(int)
plain0[LEG:] = 0
for i, f in enumerate(fr):
    m = f[..., 3] > 0
    up = m.copy(); up[LEG:] = False
    best = max(((dx, dy) for dx in range(-4, 5) for dy in range(-5, 6)),
               key=lambda d: edit.iou(shift(m0, *d), up))
    dx, dy = best if i else (0, 0)
    t = shift(top0, dx, dy)
    # Lower part: the frame's own legs; upper part: frame 0's, shifted. Below the seam the
    # body from frame 0 still wins where it is drawn, so the belly line stays one shape.
    seam = LEG + dy
    for top, name in ((t, f"walk-{i}"), (shift(plain0, dx, dy), f"walk-plain-{i}")):
        out = f.copy()
        out[:seam] = 0
        tm = top[..., 3] > 0
        out[tm] = top[tm]
        Image.fromarray(out.astype(np.uint8)).save(f"art/anim/fit/{name}.png")
    print(i, best)
