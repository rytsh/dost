"""Worn items drawn ON the dog in every pose.

The image model gets one pose of the dog (the first frame of each action, and every frame of
happy, whose head moves a lot) and adds one item to it; the take is registered back onto the
sprite grid and the item is what differs from the dog. So a hat sits right on a
sleeping, sad or dangling head too, not only on the standing one.

Run from the repo root with the Sprite Forge venv, after `make atlas`:
    PY art/fit_worn.py ref                       art/items-fit/poses/<pose>.png (references)
    PY art/fit_worn.py fit [item...]             art/items-fit/<item>-<pose>.png ->
                                                 art/items/<item>/<pose>.png and worn.json
pack_items.py packs them; the renderer picks the one for each frame's action.
"""

import json
import os
import re
import sys

import numpy as np
from PIL import Image
from scipy import ndimage

ITEMS = ["beanie", "party", "witch", "santa"]
# Pose name -> (row in the atlas, frame).
ANIMS = ["idle", "walk", "sit", "bark", "happy", "sleep", "held", "sad"]
POSES = {a: (i, 0) for i, a in enumerate(ANIMS) if a != "happy"}
POSES.update({f"happy{f}": (4, f) for f in range(4)})
S, REF = 5, 1024
ABOVE = 50  # room above the cell for hats (sprite pixels)
COLORS = 14

src = open("assets/dog_atlas.rs").read()
CW, CH = map(int, re.search(r"CELL: \(u32, u32\) = \((\d+), (\d+)\)", src).groups())
AX, AY = map(int, re.search(r"ANCHOR: \(u32, u32\) = \((\d+), (\d+)\)", src).groups())
RW, RH = CW, CH + ABOVE
OX, OY = (REF - RW * S) // 2, (REF - RH * S) // 2
atlas = np.asarray(Image.open("assets/dog.png").convert("RGBA")).astype(int)


def region(pose):
    """The pose's cell with ABOVE empty rows on top."""
    row, f = POSES[pose]
    out = np.zeros((RH, RW, 4), int)
    out[ABOVE:] = atlas[row * CH : (row + 1) * CH, f * CW : (f + 1) * CW]
    return out


def ref():
    os.makedirs("art/items-fit/poses", exist_ok=True)
    for pose in POSES:
        im = Image.fromarray(region(pose).astype(np.uint8)).resize((RW * S, RH * S), Image.NEAREST)
        c = Image.new("RGBA", (REF, REF), (255, 0, 255, 255))
        c.alpha_composite(im, (OX, OY))
        c.convert("RGB").save(f"art/items-fit/poses/{pose}.png")
    print("poses:", list(POSES))


def background(im):
    r, g, b, a = (im[..., i] for i in range(4))
    return (a < 128) | ((r > 170) & (g < 100) & (b > 170))


def sample(take, k, dx, dy):
    xs = ((OX + (np.arange(RW) + 0.5) * S) * k + dx).round().astype(int).clip(0, take.shape[1] - 1)
    ys = ((OY + (np.arange(RH) + 0.5) * S) * k + dy).round().astype(int).clip(0, take.shape[0] - 1)
    return take[ys][:, xs]


def fit_one(item, pose):
    dog = region(pose)
    dog_a = dog[..., 3] > 0
    take = np.asarray(Image.open(f"art/items-fit/{item}-{pose}.png").convert("RGBA")).astype(int)
    k0 = take.shape[0] / REF

    # The model redraws the dog slightly moved: the shift whose colours match the dog best
    # (the item covers only a small part of it).
    def score(p):
        g = sample(take, *p)
        same = (np.abs(g[..., :3] - dog[..., :3]).sum(-1) < 90) & ~background(g)
        return (same & dog_a).sum() - (~background(g) & ~dog_a).sum() * 0.1

    best = max(
        ((k0 * q, dx, dy) for q in np.linspace(0.98, 1.02, 5) for dx in range(-24, 25, 3) for dy in range(-24, 25, 3)),
        key=score,
    )
    k, bx, by = best
    best = max(((k, dx, dy) for dx in range(bx - 3, bx + 4) for dy in range(by - 3, by + 4)), key=score)
    g = sample(take, *best)
    fg = ~background(g)
    diff = np.abs(g[..., :3] - dog[..., :3]).sum(-1)
    it = fg & (~dog_a | (diff > 150))
    # The item: its biggest piece and the pieces close to it (drop specks where the redrawn
    # fur differs).
    lab, n = ndimage.label(it, structure=np.ones((3, 3)))
    sizes = ndimage.sum(it, lab, range(1, n + 1))
    main = lab == 1 + int(np.argmax(sizes))
    near = ndimage.binary_dilation(main, iterations=6)
    keep_ids = [i + 1 for i, s in enumerate(sizes) if s >= max(10, 0.04 * sizes.max()) and (near & (lab == i + 1)).any()]
    keep = np.isin(lab, keep_ids)
    keep = ndimage.binary_closing(keep, iterations=1) & fg
    keep = ndimage.binary_fill_holes(keep) & fg
    ys, xs = np.nonzero(keep)
    x0, y0, x1, y1 = xs.min(), ys.min(), xs.max() + 1, ys.max() + 1
    rgba = np.zeros((y1 - y0, x1 - x0, 4), np.uint8)
    rgba[..., :3] = g[y0:y1, x0:x1, :3]
    rgba[..., 3] = keep[y0:y1, x0:x1] * 255
    im = Image.fromarray(rgba)
    pal = im.convert("RGB").quantize(COLORS, method=Image.Quantize.MEDIANCUT).convert("RGB")
    im = Image.merge("RGBA", (*pal.split(), im.getchannel("A")))
    os.makedirs(f"art/items/{item}", exist_ok=True)
    im.save(f"art/items/{item}/{pose}.png")
    # Top-left relative to the feet point.
    at = (int(x0 - AX), int(y0 - ABOVE - AY))
    print(f"{item} {pose}: {im.size} at {at} (match {score(best):.0f})")
    return at


def fit(items):
    for item in items:
        path = f"art/items/{item}/worn.json"
        worn = json.load(open(path)) if os.path.exists(path) else {}
        for pose in POSES:
            if os.path.exists(f"art/items-fit/{item}-{pose}.png"):
                worn[pose] = fit_one(item, pose)
        json.dump(worn, open(path, "w"), indent=1)


if __name__ == "__main__":
    if sys.argv[1] == "ref":
        ref()
    else:
        fit(sys.argv[2:] or ITEMS)
