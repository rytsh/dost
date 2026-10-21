# Action sheet prompts

All generated with the master (output of `prompts/master.txt`) as `reference_images`.

## Shared part (added to every sheet)

```
The FIRST attached image is the approved master sprite of this exact character: a cute small scruffy terrier puppy with warm golden-tan fur, cream-white muzzle, chest and paw tips, one floppy dark-brown ear and one half-perked ear, large shiny dark eyes, small black nose, short fluffy upturned tail, bright red collar with a round gold tag. Copy its identity, colours, proportions, outline weight and premium 16-bit SNES-era pixel art style exactly (crisp chunky pixels, strong dark outline, 3-tone cel shading, no smooth gradients, no blur).
Every cell: the same puppy at exactly the same scale as the FIRST image, side view, clearly FACING RIGHT (head toward the RIGHT edge), resting on one shared invisible ground line near the bottom of the cell, the whole puppy (ears, tail, paws) inside the central 70 percent of its cell with a generous empty margin on every side, nothing touching or crossing a cell edge, no gutters, no separators, no grid lines. The puppy stays in place (no travel across the cell) and never turns around.
Background: solid flat pure magenta #FF00FF everywhere outside the puppy, no shadow, no ground, no dust, no motion lines, no glow, no hearts, no symbols. No text, no letters, no numbers, no labels.
```

Square sheets start with `Create ONE square image: a sprite sheet of 4 animation frames in 2 rows x 2 columns, read left to right, top row first: <action>`.

## walk (2:1, 2x4)

```
Create ONE wide image, aspect 2:1: a sprite sheet of 8 animation frames in 2 rows x 4 columns, read left to right, top row first, one walk cycle of a four-legged puppy trotting to the RIGHT.
Quadruped walk, diagonal leg pairs (NEAR = viewer-side legs, normal brightness; FAR = away-side legs, one shade darker, partly hidden):
0: NEAR front paw reaches forward, FAR hind paw reaches forward; the other two legs push back.
1: weight settles, body slightly lower, legs under the body.
2: NEAR front and FAR hind legs pass under the body; the other pair swings forward.
3: body at its highest, FAR front and NEAR hind paws just lifting forward.
4: FAR front paw reaches forward, NEAR hind paw reaches forward; the other two legs push back.
5: weight settles, body slightly lower.
6: FAR front and NEAR hind legs pass under the body; the other pair swings forward.
7: body at its highest, NEAR front and FAR hind paws just lifting forward.
Frames 4-7 must mirror the leg order of 0-3 (the leading legs swap). The head bobs a little, the ears and tail bounce slightly, the tail wags. Same body size and same ground line in all 8 frames; the puppy never turns around.
```

## idle — "a standing idle loop."

```
All four paws planted on the ground the whole time, standing like the FIRST image.
0: neutral stand, tail up.
1: inhale: chest rises a tiny bit, tail wags to one side, ears lift slightly.
2: tail wags to the other side, head tilts very slightly.
3: exhale, back to almost neutral, eyes blink half-closed.
```

## sit — "a sitting idle loop."

```
The puppy SITS on its haunches: hind legs folded under the body, rump on the ground, front legs straight and upright, chest up, head held up looking RIGHT, tail on the ground behind.
0: sitting, neutral, mouth closed.
1: sitting, tail sweeps along the ground, ears lift a little, mouth slightly open panting with pink tongue.
2: sitting, tail sweeps back, head tilts a little.
3: sitting, eyes closed in a blink, otherwise like frame 0.
```

## sleep — "a sleeping loop."

```
The puppy LIES DOWN flat on its belly, curled a little, chin resting on its front paws, eyes CLOSED, ears relaxed down, tail curled along the body. A low wide silhouette.
0: lying asleep, body neutral.
1: inhale: back and belly rise slightly.
2: fully inhaled, ear twitches a tiny bit.
3: exhale: back lowers again. Eyes closed in every frame.
```

## bark — "a sitting bark (alert, telling the user something)."

```
The puppy SITS on its haunches (hind legs folded, rump on the ground, front legs upright) like a sitting dog, alert, both ears perked UP, tail wagging fast, looking up-RIGHT.
0: sitting alert, mouth closed, ears up.
1: head pulls back slightly, mouth begins to open.
2: BARK: head thrust forward-up, mouth wide open, front paws stay on the ground.
3: mouth closing, head returns, tail swept to the other side.
```

## happy — "a happy excited hop in place."

```
The happy puppy hops on the spot, tongue out, eyes squeezed happily, tail wagging fast. Keep the same body scale; the jump height is small (about 15 percent of the cell height).
0: crouch: front paws down, rump up, play-bow, tail high.
1: spring: all four paws leave the ground, body raised.
2: peak: in the air, paws tucked, ears flying up.
3: land: paws back on the ground line, ears flopping down.
```

## held — "the puppy being lifted up by an invisible hand (dangling)."

```
The puppy hangs in the air as if lifted by the scruff of the neck (do NOT draw a hand): body vertical-ish and relaxed, legs dangling straight down, tail hanging down, a surprised but cute face, mouth slightly open. Keep it inside the central 70 percent of the cell; there is no ground under it.
0: dangling, legs straight down.
1: legs paddle: front paws kick forward a little.
2: dangling, slight swing to the other side, hind paws kick.
3: dangling, legs straight, eyes blink.
```
