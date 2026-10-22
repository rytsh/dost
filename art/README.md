# Sprites

The dog was generated with the Sprite Forge skills (`sprite-forge-mcp`, `generate2dsprite`) through an
image model (gpt-image). The pixel-art look is applied after generation, not prompted.

1. **Master still**: generated from `prompts/master.txt`, approved with `master_still.py approve`
   (`master/master_rgba.png`, `master/master.json`).
2. **Actions**: without an image-to-video model, every action was generated as its own sheet with the
   master as the reference image (`prompts/sheets.md`). Walk is 2×4, the others 2×2.
3. **Scale and ground**: `scale_frames.py` (walk 1.0, the rest 0.7065; one ground line).
4. **Pixel finish**: `finish_frames.py pixel --outline selective`. Idle with
   `--target-height 128 --colors 40`, the others with
   `--scale-ref art/px/idle --no-area-norm --palette art/px/idle/palette.json` (one scale, one palette)
   → `px/<action>/frames`. The dog is 128 px tall.
5. **Atlas**: `make atlas` (`pack_atlas.py`) → `assets/dog.png` and `assets/dog_atlas.rs`.
   It also finds the head (matched against idle's first frame) and the collar in every frame
   (`HEAD`, `NECK`), so worn items follow the dog.
   It registers every frame to whole pixels: frames are aligned to their action's first frame by
   silhouette overlap, actions are aligned to idle, the anchor is put at the body's centre (so turning
   around does not jump) and the walk stride is measured (so paws do not slide).

**sad** (2×2, sitting with ears down) was made the same way, with a sit frame as the reference.

**Toys** (bone, pumpkin) come from one 2×4 prop sheet (`prompts/items.txt`, the idle frame as the
style reference), sliced into `items-hd/<item>`, pixel-finished on their own palette
(`finish_frames.py pixel --target-height <h> --colors 14`) → `items/<item>/frames`.

**Worn items** (beanie, party hat, witch hat, Santa hat) are drawn by the image
model *on* the dog, in every pose: `fit_worn.py ref` writes each action's first frame (and every
frame of happy, whose head moves a lot) enlarged onto a magenta canvas (`items-fit/poses/`); each
item is one edit of each pose ("put a ... on its head, worn for this pose"), saved as
`items-fit/<item>-<pose>.png`. `fit_worn.py fit` registers each take back onto the sprite's grid,
keeps what differs from the dog and writes `items/<item>/<pose>.png` and `items/<item>/worn.json`
(where each goes, relative to the feet point). So a hat sits on a sleeping or a drooping head; within an action the item follows the head (`HEAD`).

`pack_items.py` (part of `make atlas`) packs all items into `assets/items.png` with where each one
goes in each pose (`assets/items_atlas.rs`).

Fur and collar colours are not separate art: the renderer shifts the hue and lightness of the fur
and collar pixels (`src/look.rs`). Which pixels are the collar comes from `collar.py` (part of
`make atlas`) → `assets/collar.png`: the band and tag in every frame (found around `NECK`, so the
tongue and ears keep their red). Where the collar was, the dog *without* a collar shows the same
frame drawn by the image model without one: each action's base frame was edited ("remove the
collar") and registered back (`art/anim/fit/nocollar-*`); `art/anim/edit.py build` writes those
frames to `px/<action>/plain`, and `pack_atlas.py` moves them along with the frames.

## Smooth animation (`art/anim`)

Frames drawn one by one by an image model "boil": the fur is redrawn differently every frame. So
each action has one base frame, and the motion is image edits of it that change one part (tail
wag, half-closed and closed eyes (both: the far eye is redone on a close-up of the head, `head.py`), mouth, ears, paws), with in-betweens. `edit.py fit` registers a
take onto the sprite's pixel grid and palette (`art/anim/fit/<name>.png`); `edit.py build` pastes
only what changed, inside a box, onto the base frame, as listed in `art/anim/frames.json`. The walk
(`walk.py`) keeps the first frame's head, back and tail on every frame, moved by its bob, and the
legs from each frame. The base frames are `art/anim/base.png` (the atlas before this, with
`base_atlas.rs`). Frame timing is in `src/dog.rs` (`durations`). After a change:
`edit.py build`, then `make atlas`.

Raw generations, HD frames and QC reports are not in git (`.gitignore`). To regenerate an action,
repeat steps 2–5 for it.
