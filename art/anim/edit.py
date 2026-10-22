"""Smooth animation frames from image edits of one base frame per action.

An image model redraws the whole picture on every edit, so independently drawn frames "boil"
(the fur changes from frame to frame). Here each action has one base frame; the model makes
edits of it that change one thing (the tail, the eyes, the mouth...), each edit is registered
back onto the sprite's pixel grid, and only the part that changed, inside a box, is pasted onto
the base. Everything else stays the base's pixels, so frames differ only where they move.

Run from the repo root with the Sprite Forge venv:
    PY art/anim/edit.py ref <anim> <frame> <out.png>      reference for generate_image
    PY art/anim/edit.py fit <take.png> <anim> <frame> <name> [x0 y0 x1 y1]
        registers a take of that cell (the box, if given, is left out of the fit) ->
        art/anim/fit/<name>.png (cell-sized, on the palette)
    PY art/anim/edit.py build                             art/anim/frames.json -> art/px/*/frames

Cells are those of art/anim/base.png (a copy of the atlas the frames start from, with base_atlas.rs); boxes are in cell pixels.
"""

import json
import os
import re
import sys

import numpy as np
from PIL import Image
from scipy import ndimage

S, REF = 6, 1024
ANIMS = ["idle", "walk", "sit", "bark", "happy", "sleep", "held", "sad"]
src = open("art/anim/base_atlas.rs").read()  # the cells of base.png
CW, CH = map(int, re.search(r"CELL: \(u32, u32\) = \((\d+), (\d+)\)", src).groups())
AX, AY = map(int, re.search(r"ANCHOR: \(u32, u32\) = \((\d+), (\d+)\)", src).groups())
FR = [int(v) for v in re.search(r"FRAMES: \[usize; \d+\] = \[([^\]]*)\]", src).group(1).split(",")]
OX, OY = (REF - CW * S) // 2, (REF - CH * S) // 2
PALETTE = np.array(
    [[int(c["hex"][i : i + 2], 16) for i in (1, 3, 5)] for c in json.load(open("art/px/idle/palette.json"))["colors"]]
)
ATLAS_PNG = os.environ.get("DOG_ATLAS", "art/anim/base.png")


def atlas():
    return np.asarray(Image.open(ATLAS_PNG).convert("RGBA")).astype(int)


def cell(a, anim, frame):
    row = ANIMS.index(anim)
    return a[row * CH : (row + 1) * CH, frame * CW : (frame + 1) * CW].copy()


def to_palette(rgb):
    d = ((rgb[..., None, :] - PALETTE[None, None]) ** 2).sum(-1)
    return PALETTE[d.argmin(-1)]


def background(im):
    r, g, b, a = (im[..., i] for i in range(4))
    return (a < 128) | ((r > 170) & (g < 100) & (b > 170))


def sample(take, k, dx, dy):
    xs = ((OX + (np.arange(CW) + 0.5) * S) * k + dx).round().astype(int).clip(0, take.shape[1] - 1)
    ys = ((OY + (np.arange(CH) + 0.5) * S) * k + dy).round().astype(int).clip(0, take.shape[0] - 1)
    return take[ys][:, xs]


def iou(a, b):
    return (a & b).sum() / max((a | b).sum(), 1)


def ref(anim, frame, out):
    c = cell(atlas(), anim, frame).astype(np.uint8)
    im = Image.fromarray(c).resize((CW * S, CH * S), Image.NEAREST)
    canvas = Image.new("RGBA", (REF, REF), (255, 0, 255, 255))
    canvas.alpha_composite(im, (OX, OY))
    canvas.convert("RGB").save(out)


def fit(take_path, anim, frame, name, box=None, quiet=False):
    base = cell(atlas(), anim, frame)
    take = np.asarray(Image.open(take_path).convert("RGBA")).astype(int)
    k0 = take.shape[0] / REF
    base_a = base[..., 3] > 0
    use = np.ones_like(base_a)
    if box:
        x0, y0, x1, y1 = box
        use[max(0, y0) : y1, max(0, x0) : x1] = False

    def score(p):
        return iou(~background(sample(take, *p)) & use, base_a & use)

    best = max(
        ((k0 * s, dx, dy) for s in np.linspace(0.96, 1.04, 17) for dx in range(-30, 31, 3) for dy in range(-30, 31, 3)),
        key=score,
    )
    k, bx, by = best
    best = max(((k, dx, dy) for dx in range(bx - 3, bx + 4) for dy in range(by - 3, by + 4)), key=score)
    g = sample(take, *best)
    fg = ~background(g)
    out = np.zeros_like(base)
    out[..., :3] = to_palette(g[..., :3])
    out[..., 3] = fg * 255
    os.makedirs("art/anim/fit", exist_ok=True)
    Image.fromarray(out.astype(np.uint8)).save(f"art/anim/fit/{name}.png")
    if not quiet:
        print(f"{name}: fit {score(best):.3f} (scale {best[0] / k0:.3f}, {best[1]:+d}, {best[2]:+d})")


def patch(base, fitted, box, thr=90, plain=None):
    """Pastes what changed inside `box` from `fitted` onto `base`. With `plain` (the same frame
    without the collar) the same pixels go onto it too; returns (base, plain)."""
    x0, y0, x1, y1 = box
    inside = np.zeros(base.shape[:2], bool)
    inside[max(0, y0) : y1, max(0, x0) : x1] = True
    ba, fa = base[..., 3] > 0, fitted[..., 3] > 0
    diff = (ba != fa) | (ba & fa & (np.abs(base[..., :3] - fitted[..., :3]).sum(-1) > thr))
    diff &= inside
    # Keep the real changes (drop specks where the redrawn fur differs), as whole shapes.
    lab, n = ndimage.label(diff, structure=np.ones((3, 3)))
    if n == 0:
        return base, plain
    sizes = ndimage.sum(diff, lab, range(1, n + 1))
    keep = np.isin(lab, [i + 1 for i, s in enumerate(sizes) if s >= max(6, 0.04 * sizes.max())])
    keep = ndimage.binary_closing(keep, iterations=2)
    keep = ndimage.binary_fill_holes(keep)
    keep = ndimage.binary_dilation(keep, iterations=1) & inside
    out = base.copy()
    out[keep] = fitted[keep]
    out[..., 3] = np.where(out[..., 3] > 0, 255, 0)
    if plain is not None:
        plain = plain.copy()
        plain[keep] = fitted[keep]
        plain[..., 3] = np.where(plain[..., 3] > 0, 255, 0)
    return out, plain


def load_fit(name):
    return np.asarray(Image.open(f"art/anim/fit/{name}.png").convert("RGBA")).astype(int)


def build():
    """Writes art/px/<anim>/frames and, from the collar-less takes, art/px/<anim>/plain (the
    same frames without the collar; pack_atlas.py turns them into assets/collar.png's fill)."""
    spec = json.load(open("art/anim/frames.json"))
    a = atlas()
    for anim, frames in spec.items():
        if anim.startswith("_"):
            continue
        for d in ("frames", "plain"):
            out_dir = f"art/px/{anim}/{d}"
            os.makedirs(out_dir, exist_ok=True)
            for f in os.listdir(out_dir):
                os.remove(os.path.join(out_dir, f))
        for i, fr in enumerate(frames):
            if "file" in fr:
                img, plain = load_fit(fr["file"]), load_fit(fr["plain"])
            else:
                b = fr.get("base", 0)
                img = cell(a, anim, b)
                plain = load_fit(f"nocollar-{anim}" + (str(b) if anim == "happy" else ""))
            for p in fr.get("patches", []):
                img, plain = patch(img, load_fit(p["fit"]), p["box"], p.get("thr", 90), plain)
            Image.fromarray(img.astype(np.uint8)).save(f"art/px/{anim}/frames/frame-{i:02d}.png")
            Image.fromarray(plain.astype(np.uint8)).save(f"art/px/{anim}/plain/frame-{i:02d}.png")
        fin = json.load(open(f"art/px/{anim}/finish.json"))
        fin["anchor"], fin["size"] = [AX, AY], [CW, CH]
        json.dump(fin, open(f"art/px/{anim}/finish.json", "w"), indent=2)
        print(f"{anim}: {len(frames)} frames")


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "ref":
        ref(sys.argv[2], int(sys.argv[3]), sys.argv[4])
    elif cmd == "fit":
        box = list(map(int, sys.argv[6:10])) if len(sys.argv) > 6 else None
        fit(sys.argv[2], sys.argv[3], int(sys.argv[4]), sys.argv[5], box)
    elif cmd == "build":
        build()
