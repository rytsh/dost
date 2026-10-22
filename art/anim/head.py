"""Edits of a close-up: a box of a cell-sized frame, enlarged, so the model sees small details
(the far eye). Run from the repo root with the Sprite Forge venv:
    PY art/anim/head.py ref <frame.png> x0 y0 x1 y1 <out.png>
    PY art/anim/head.py fit <frame.png> x0 y0 x1 y1 <take.png> <out.png>
`fit` registers the take back onto the box (fitted on the box's outline and fur, the eyes
change) and writes the frame with the box replaced, on the palette."""
import sys
import numpy as np
from PIL import Image
sys.path.insert(0, "art/anim")
import edit

REF = 1024


def scale(box):
    x0, y0, x1, y1 = box
    return REF // max(x1 - x0, y1 - y0)


def ref(frame, box, out):
    x0, y0, x1, y1 = box
    s = scale(box)
    im = Image.open(frame).convert("RGBA").crop(box).resize(((x1 - x0) * s, (y1 - y0) * s), Image.NEAREST)
    c = Image.new("RGBA", (REF, REF), (255, 0, 255, 255))
    c.alpha_composite(im, ((REF - im.width) // 2, (REF - im.height) // 2))
    c.convert("RGB").save(out)


def fit(frame, box, take_path, out):
    x0, y0, x1, y1 = box
    s = scale(box)
    w, h = x1 - x0, y1 - y0
    ox, oy = (REF - w * s) // 2, (REF - h * s) // 2
    base = np.asarray(Image.open(frame).convert("RGBA")).astype(int)
    sub = base[y0:y1, x0:x1]
    take = np.asarray(Image.open(take_path).convert("RGBA")).astype(int)
    k0 = take.shape[0] / REF

    def sample(k, dx, dy):
        xs = ((ox + (np.arange(w) + 0.5) * s) * k + dx).round().astype(int).clip(0, take.shape[1] - 1)
        ys = ((oy + (np.arange(h) + 0.5) * s) * k + dy).round().astype(int).clip(0, take.shape[0] - 1)
        return take[ys][:, xs]

    dark = (sub[..., 3] > 0) & (sub[..., :3].sum(-1) < 150)

    def score(p):
        g = sample(*p)
        gd = ~edit.background(g) & (g[..., :3].sum(-1) < 150)
        return edit.iou(gd, dark)

    best = max(((k0 * q, dx, dy) for q in np.linspace(0.97, 1.03, 7) for dx in range(-40, 41, 4) for dy in range(-40, 41, 4)), key=score)
    k, bx, by = best
    best = max(((k, dx, dy) for dx in range(bx - 4, bx + 5) for dy in range(by - 4, by + 5)), key=score)
    g = sample(*best)
    res = base.copy()
    fg = ~edit.background(g)
    piece = np.zeros_like(sub)
    piece[..., :3] = edit.to_palette(g[..., :3])
    piece[..., 3] = fg * 255
    res[y0:y1, x0:x1] = piece
    Image.fromarray(res.astype(np.uint8)).save(out)
    print(f"{out}: fit {score(best):.3f}")


if __name__ == "__main__":
    box = tuple(map(int, sys.argv[3:7]))
    if sys.argv[1] == "ref":
        ref(sys.argv[2], box, sys.argv[7])
    else:
        fit(sys.argv[2], box, sys.argv[7], sys.argv[8])
