"""art/px/<anim>/frames -> assets/dog.png (one row per animation, one shared cell and anchor).

Run from the repo root with the Sprite Forge venv (or `make atlas`):
    ~/tools/agent-sprite-forge/.venv/bin/python art/pack_atlas.py

Every frame is registered to its animation's first frame so the body does not swim:
- x: the whole-pixel shift that best overlaps the silhouette (IoU) with frame 0;
- y: grounded animations put their lowest pixel on the ground line, `happy` keeps its
  drawn hop, `held` (dangling, no ground) aligns its top half to frame 0.
Prints the cell, the anchor and the walk stride; they must match src/render.rs and src/dog.rs.
"""

import glob
import json

import numpy as np
from PIL import Image

# Row order must match `Anim` in src/dog.rs.
ANIMS = ["idle", "walk", "sit", "bark", "happy", "sleep", "held", "sad"]
GROUNDED = {"idle", "walk", "sit", "bark", "sleep", "sad"}
# Shift search ranges, relative to sprite height (14 px for a 64 px dog).
SCALE = json.load(open("art/px/idle/finish.json"))["targetHeight"] / 64
SEARCH = round(14 * SCALE)
MARGIN = 1


def load(anim):
    """Frames as 8 channels: the frame's RGBA, then the same frame without the collar
    (art/px/<anim>/plain, drawn by the image model), so both get the same shifts."""
    fin = json.load(open(f"art/px/{anim}/finish.json"))
    ax, ay = fin["anchor"]
    frames = []
    for path in sorted(glob.glob(f"art/px/{anim}/frames/*.png")):
        im = np.asarray(Image.open(path).convert("RGBA"))
        plain = path.replace("/frames/", "/plain/")
        pl = np.asarray(Image.open(plain).convert("RGBA")) if glob.os.path.exists(plain) else im
        frames.append((np.concatenate([im, pl], -1), ax, ay))
    return frames


def place(frames, pad=round(48 * SCALE)):
    """Put every frame of one animation on a shared canvas, anchors on one point."""
    left = max(ax for _, ax, _ in frames) + pad
    top = max(ay for _, _, ay in frames) + pad
    w = left + max(im.shape[1] - ax for im, ax, _ in frames) + pad
    h = top + max(im.shape[0] - ay for im, _, ay in frames) + pad
    out = []
    for im, ax, ay in frames:
        c = np.zeros((h, w, im.shape[2]), np.uint8)
        c[top - ay : top - ay + im.shape[0], left - ax : left - ax + im.shape[1]] = im
        out.append(c)
    return out, (left, top)


def shift(img, dx, dy):
    out = np.zeros_like(img)
    h, w = img.shape[:2]
    ys, yd = (slice(0, h - dy), slice(dy, h)) if dy >= 0 else (slice(-dy, h), slice(0, h + dy))
    xs, xd = (slice(0, w - dx), slice(dx, w)) if dx >= 0 else (slice(-dx, w), slice(0, w + dx))
    out[yd, xd] = img[ys, xs]
    return out


def iou(a, b):
    u = np.logical_or(a, b).sum()
    return np.logical_and(a, b).sum() / u if u else 0.0


def lowest_row(mask):
    return np.nonzero(mask.any(1))[0].max()


def register(anim, canvases, anchor):
    ref = canvases[0][..., 3] > 0
    ground = anchor[1] - 1  # lowest opaque row sits just above the feet line
    out = []
    for i, c in enumerate(canvases):
        m = c[..., 3] > 0
        if anim == "held":
            rows = np.nonzero(ref.any(1))[0]
            cut = (rows.min() + rows.max()) // 2
            top_ref = ref.copy()
            top_ref[cut:] = False
            best = max(
                ((dx, dy) for dx in range(-SEARCH, SEARCH + 1) for dy in range(-SEARCH, SEARCH + 1)),
                key=lambda d: iou(top_ref, shift(m, *d) & top_ref.any(1)[:, None]),
            )
            dx, dy = best if i else (0, 0)
        else:
            dy = ground - lowest_row(m) if anim in GROUNDED else 0
            m2 = shift(m, 0, dy)
            dx = max(range(-SEARCH, SEARCH + 1), key=lambda d: iou(ref, shift(m2, d, 0))) if i else 0
        out.append(shift(c, dx, dy))
    return out


def walk_stride(frames, ground):
    """Median backward travel (px/frame) of the paws on the ground line, after registration."""
    moves = []
    n = len(frames)
    for i in range(n):
        a = frames[i][ground] [..., 3] > 0
        b = frames[(i + 1) % n][ground][..., 3] > 0
        best = max(range(0, round(12 * SCALE)), key=lambda d: (np.roll(b, d) & a).sum())
        if (np.roll(b, best) & a).sum() > 2 * SCALE:
            moves.append(best)
    return float(np.median(moves)) if moves else 0.0


registered = {}
for anim in ANIMS:
    canvases, anchor = place(load(anim))
    registered[anim] = (register(anim, canvases, anchor), anchor)

# Line every animation up with idle (head and chest overlap best) so switching animations
# does not make the body jump sideways. Sleep and held have other silhouettes; keep them.
idle_frames, (iax, iay) = registered["idle"]
idle0 = idle_frames[0][..., 3] > 0
for anim in ["walk", "sit", "bark", "happy", "sad"]:
    frames, (ax, ay) = registered[anim]
    m = frames[0][..., 3] > 0
    ref = np.zeros_like(m)
    h = min(ref.shape[0] - ay, idle0.shape[0] - iay)
    w0 = min(ax, iax)
    w1 = min(ref.shape[1] - ax, idle0.shape[1] - iax)
    top_cut = min(ay, iay)
    ref[ay - top_cut : ay + h, ax - w0 : ax + w1] = idle0[iay - top_cut : iay + h, iax - w0 : iax + w1]
    rows = np.nonzero(ref.any(1))[0]
    upper = np.zeros(ref.shape[0], bool)
    upper[: (rows.min() + rows.max()) // 2] = True  # head and chest only
    ref &= upper[:, None]
    dx = max(range(-SEARCH, SEARCH + 1), key=lambda d: iou(ref, shift(m, d, 0) & upper[:, None]))
    registered[anim] = ([shift(f, dx, 0) for f in frames], (ax, ay))
    print(f"{anim}: shifted {dx:+d} px to line up with idle")

# Put the anchor on idle's body centre, so turning around (mirroring about the anchor) does not jump.
ys, xs = np.nonzero(idle0)
centre = (xs.min() + xs.max() + 1) // 2 - iax
for anim, (frames, a) in registered.items():
    registered[anim] = ([shift(f, -centre, 0) for f in frames], a)
print(f"anchor moved {centre:+d} px to the body centre")

# One cell for every animation: the union of opaque pixels around each anchor.
ext = [0, 0, 0, 0]  # left, top, right, bottom around the anchor
for frames, (ax, ay) in registered.values():
    for f in frames:
        ys, xs = np.nonzero(f[..., 3] > 0)
        ext = [max(ext[0], ax - xs.min()), max(ext[1], ay - ys.min()),
               max(ext[2], xs.max() + 1 - ax), max(ext[3], ys.max() + 1 - ay)]
left, top = ext[0] + MARGIN, ext[1] + MARGIN
cw, ch = left + ext[2] + MARGIN, top + ext[3] + MARGIN
cols = max(len(f) for f, _ in registered.values())

both = np.zeros((ch * len(ANIMS), cw * cols, 8), np.uint8)
for row, anim in enumerate(ANIMS):
    frames, (ax, ay) = registered[anim]
    for col, f in enumerate(frames):
        both[row * ch : (row + 1) * ch, col * cw : (col + 1) * cw] = f[ay - top : ay - top + ch, ax - left : ax - left + cw]
atlas = np.ascontiguousarray(both[..., :4])
Image.fromarray(atlas).save("assets/dog.png", optimize=True)
# The dog without its collar, for collar.py (not shipped).
glob.os.makedirs("target", exist_ok=True)
Image.fromarray(np.ascontiguousarray(both[..., 4:])).save("target/dog-plain.png")

# Where the head and the collar are in every cell, so hats
# follow the dog. The head is found by matching idle's first head against each frame.
def cells():
    for row, anim in enumerate(ANIMS):
        for col in range(len(registered[anim][0])):
            yield atlas[row * ch : (row + 1) * ch, col * cw : (col + 1) * cw].astype(np.int32)


idle0_cell = atlas[0:ch, 0:cw].astype(np.int32)
HEAD_BOX = (left + 4, top - 118, left + 50, top - 66)  # x0, y0, x1, y1 around idle's head
hx0, hy0, hx1, hy1 = HEAD_BOX
patch = idle0_cell[hy0:hy1, hx0:hx1]
pmask = patch[..., 3] > 0
R = round(44 * SCALE / 2)


def head_offset(c):
    best = (1e18, 0, 0)
    h, w = hy1 - hy0, hx1 - hx0
    for dy in range(-R, R + 1):
        for dx in range(-R, R + 1):
            ya, xa = hy0 + dy, hx0 + dx
            if ya < 0 or xa < 0 or ya + h > ch or xa + w > cw:
                continue
            q = c[ya : ya + h, xa : xa + w]
            d = np.abs(q[..., :3] - patch[..., :3]).sum(-1) * pmask
            d = d + (pmask != (q[..., 3] > 0)) * 300
            s = d.mean()
            if s < best[0]:
                best = (s, dx, dy)
    return best[1], best[2]


def collar(c):
    r, g, b, a = c[..., 0], c[..., 1], c[..., 2], c[..., 3]
    ys, xs = np.nonzero((a > 0) & (r > 150) & (g < 70) & (b < 60))
    return (round(xs.mean()) - left, round(ys.mean()) - top) if len(xs) else (0, -60)


heads = [head_offset(c) for c in cells()]
necks = [collar(c) for c in cells()]

walk, (wax, way) = registered["walk"]
stride = walk_stride(walk, way - 1)
counts = ", ".join(str(len(registered[a][0])) for a in ANIMS)
def fmt(pts):
    return "[" + ", ".join(f"({x}, {y})" for x, y in pts) + "]"


with open("assets/dog_atlas.rs", "w") as f:
    f.write(
        "// Generated by art/pack_atlas.py (make atlas). Do not edit.\n"
        f"/// Cell size of `assets/dog.png`, sprite pixels.\npub const CELL: (u32, u32) = ({cw}, {ch});\n"
        f"/// Feet point (ground line, body centre) inside a cell.\npub const ANCHOR: (u32, u32) = ({left}, {top});\n"
        f"/// Frames per row, in `Anim` order: {', '.join(ANIMS)}.\npub const FRAMES: [usize; {len(ANIMS)}] = [{counts}];\n"
        f"/// Measured paw travel per walk frame, sprite pixels.\npub const WALK_STRIDE: f32 = {stride:.1f};\n"
        f"/// How far the head moved from idle's first frame, per cell in row order (sprite pixels).\n"
        f"pub const HEAD: [(i16, i16); {len(heads)}] = {fmt(heads)};\n"
        f"/// Centre of the collar relative to the feet point, per cell in row order (sprite pixels).\n"
        f"pub const NECK: [(i16, i16); {len(necks)}] = {fmt(necks)};\n"
    )

print(f"CELL = ({cw}, {ch}), ANCHOR = ({left}, {top}), walk stride ~ {stride} px/frame")
print("frames:", {a: len(registered[a][0]) for a in ANIMS})
