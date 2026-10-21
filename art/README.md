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

**Items** (bone, beanie, party hat, witch hat, Santa hat, bandana, sunglasses, pumpkin) are one
2×4 prop sheet (`prompts/items.txt`, the idle frame as the style reference), sliced into
`items-hd/<item>`, pixel-finished on their own palette (`finish_frames.py pixel --target-height
<h> --colors 14`) → `items/<item>/frames`. `pack_items.py` (part of `make atlas`) packs them into
`assets/items.png` with where each one pins onto the dog (`assets/items_atlas.rs`).

Fur and collar colours are not separate art: the renderer shifts the hue and lightness of the fur
and collar pixels (`src/look.rs`).

Raw generations, HD frames and QC reports are not in git (`.gitignore`). To regenerate an action,
repeat steps 2–5 for it.
