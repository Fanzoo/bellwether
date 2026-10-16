# Bellwether's own pets: the specification

This is the only brief the pets are drawn from (distribution plan D8). It is written in
words on purpose: no reference image, and no fetched or imported pet, is an input to the
art. Everything in `pets/` is CC0-1.0 (`pets/LICENSE`). `scripts/build-pet.mjs` and the
tests are code, under the repository's MIT licence.

Two pets, both wearing a pilot's **flight helmet with goggles**. On both, the **helmet**
carries the session colour, so the strip looks the same whichever pet is chosen:

| Id | What it is | Status |
|---|---|---|
| `bellwether` | an original sheep in a flight helmet; the default (`pets.default` absent means `bellwether`) | M10a, required for 1.0 |
| `brah` | an aviator goose: a white goose in a flight helmet with goggles, a little cheeky | M10b, after 1.0 |

`bellwether` and `brah` are reserved names: `bellwether pets import` and the setup
window's Import… refuse them. The product stays Bellwether, and "sheep" and "lamb" stay
the general terms; the goose's subagents may be called goslings where that reads better.
One pet draws the whole strip; mixing pets per session is out of scope.

## The source form

What you edit to change the art. Everything is plain text, so it can be edited by hand,
read in a diff and rebuilt. Per pet:

```
pets/<name>/
  pet.json                the animation table, the behaviour map and the title
  src/palette.json        every colour the pet uses
  src/parts/<part>.txt    reusable pieces (a head, a body, a leg)
  src/frames/<anim>_<nn>.txt   one frame each
  animations.xml          BUILT: never edit it; scripts/build-pet.mjs writes it
```

### palette.json

```json
{ "colours": [ { "char": "2", "hex": "#b300b3", "role": "mask-mid" }, … ] }
```

- `char`: one printable ASCII character, not `.`, `~` or `#` (those mean transparent,
  erase and comment).
- `hex`: `#rrggbb`. Never `#ff00ff`: the sheet has real alpha and no key colour.
- `role`: `mask-dark`, `mask-mid` or `mask-light` (exactly `#730073`, `#b300b3` and
  `#ff7aff`; all three are required), `outline`, or a free lower-case name such as
  `wool`, `face`, `feather`, `beak`, `goggle-lens`, `goggle-frame` or `strap`.
- A colour that isn't a mask tone may not fall in the tinter's window (hue 285–315°,
  saturation ≥ 0.35): it would take the session colour. The build refuses it.

### Grids

A grid is rows of palette characters, all the same length. `.` is transparent. Text
after `#` on a line is a comment, and blank lines are skipped. In parts and overlays, `~`
erases what is below it to transparent.

### Parts

`src/parts/<part>.txt` is a grid of any rectangular size. Parts don't refer to other
parts.

### Frames

`src/frames/<anim>_<nn>.txt` is one 40 × 40 frame, in one of two forms:

1. **A grid**: exactly 40 rows of 40 characters.
2. **A composition**: lines that are layered in order onto an empty tile.

   | Line | Meaning |
   |---|---|
   | `part <part> <x> <y>` | paints `parts/<part>.txt` with its top-left pixel at column `x`, row `y` |
   | `part <part> <x> <y> flip` | the same, mirrored left to right first |
   | `grid <x> <y>` … `end` | an overlay grid, its rows on the lines between |
   | `shift <dx> <dy>` | moves everything painted so far |

   Columns and rows count from 0 at the top left. `.` in a part or overlay leaves what is
   below, and `~` erases it. A pixel painted or shifted outside the tile is an error, not
   clipped, so the feet can't silently leave row 39.

Every frame must resolve to a 40 × 40 grid of palette characters. A frame file that no
animation uses is an error.

A generator script that writes these files is allowed if it is committed and
`build-pet --check` proves its output equals the committed text. The text files are the
source of truth.

### pet.json

```json
{
  "title": "Bellwether sheep",
  "draft": "optional: why the behaviour map doesn't yet name every animation",
  "animations": {
    "walk": { "frames": ["walk_01", "walk_02", "walk_03", "walk_02"], "interval": 140, "x": -2, "repeat": "10", "next": { "walk": 100 } },
    "sleep": { "frames": ["sleep_01", "…"], "interval": [220, 420], "next": { "sleep loop": 100 } }
  },
  "behaviours": { "walk": "walk", "sleep": ["sleep", "sleep loop"], "…": "…" }
}
```

- `animations`, in the order they get ids 1, 2, …: `frames` (frame file names; a name
  may repeat), and optionally `interval` (ms), `x`, `y` and `offsety` (each a number, or
  `[start, end]` interpolated across the run, as desktopPet does), `repeat` (an
  expression, default `"0"`), `repeatfrom` (default 0), and `next` (animation name →
  probability).
- `behaviours`: all 16 behaviour keys, each to an animation name or a list of them.
- `overlays` (optional): frame name → a list of `{ "part": <part>, "x": <int>, "y": <int> }`.
  Each is a small image the strip draws over that frame and **never mirrors**: placed as
  for a pet facing left, it moves to the mirrored position when the pet faces right, but
  the image itself stays the right way round. Use it for marks that must read the same
  both ways, such as letters. The build packs each overlay part into its own extra tile
  after the frames (the file stays plain desktopPet; other players just see unused
  tiles) and pins `{ "<frame index>": [{ tile, x, y, w, h }] }` under the pet's `overlays`
  in `pets.lock.json`. The strip reads them from the pin (`shell/src/player/overlay.mjs`),
  so an imported pet has none. The acceptance checks measure frames as shown, overlays
  included.
- `title`: the header title.

### Building

```
node scripts/build-pet.mjs build <name>       sources -> animations.xml and its pin
node scripts/build-pet.mjs build --all
node scripts/build-pet.mjs --check            in npm test and CI: fails if either differs
node scripts/build-pet.mjs review <name> [--out <dir>]   the review sheets (acceptance f)
```

The build resolves every frame, refuses any character not in the palette, any mask role
in another colour and any other colour inside the hue window, packs the sheet 16 tiles
wide, encodes the PNG deterministically (an 8-bit RGBA PNG, one fixed-Huffman deflate
block, no time, gamma or text chunks), writes a desktopPet-format `animations.xml` with the
PNG embedded as base64, and records `{sha256, bytes, animationMap}` under `bundled` in
`pets.lock.json`. A rebuild from the same sources is byte-identical.

## The shared rules (both pets)

- **Animations**, named after Bellwether's behaviours: `walk`, `run`, `graze` and `chew`,
  `sleep` (enter) and `sleep loop`, `alert`, `turn`, `trip`, `sit`, `dizzy`, `arrive`,
  `leave`, `hop`, `lie`. `arrive` and `leave` may reuse walk and run frames; `chew` may
  reuse graze frames.
- **`carried` and `land`**, for the planned drag along the strip (a later milestone; the
  strip doesn't play them yet). `carried`: the pet dangling while held from above, at
  about the helmet or scruff, legs kicking gently, looping, 2–4 frames. `land`: a short
  drop and settle, 2–4 frames, ending in a pose that flows into walk or graze. A pet
  without them (any imported pet) falls back to walk.
- **The alert.** The player plays `turn`, holds its **last frame** and bounces it, so
  `turn` ends on the **front-facing pose**. `alert` is the one-off escalation when an
  alert is confirmed (a roll or a bigger bounce); it ends on a side frame, after which
  `turn` plays again.
- **Indicative sizes:** walk 4 frames, run 4, turn 3; alert 6–10; graze 4–6, sleep 4 and
  a loop of 2–4; trip, hop and dizzy 4–6 each; sit and lie 2–4 each; carried and land 2–4
  each. About 50–60 unique frames per pet.
- **Behaviour map.** Each of the 16 keys maps to the animation of the same name, except
  `sleep`, which maps to `["sleep", "sleep loop"]`. The build copies the map into the pin;
  the strip uses it when config.json has no `pets.animationMap.<name>`.
- **Frames face left.** Walk moves `x` negative; the player mirrors for rightward
  movement.
- **Tile: 40 × 40.** Feet stand on row 39, the tile's bottom row, so the strip's ground
  line holds. Fitting the helmet into 40 rows is a review point, not a reason to change
  the tile.
- **The colour mask is the helmet.** The helmet (shell, ear flaps, chin strap) is drawn
  only in the three mask tones: dark `#730073`, mid `#b300b3` (the session colour lands
  there exactly), light `#ff7aff`. Nothing else may fall in the tinter's window. Goggles,
  wool, feathers, face, beak, eyes, legs and feet are never mask tones. The helmet shows
  in every frame of every mapped animation (the tests ask for at least 8 mask pixels, 4 of
  them the mid tone).
- **Contrast with the session colour** (CIEDE2000, for each of the 8 default palette
  colours tinted through the strip's tinter): dark edge colours (the outline, or any
  colour with L* ≤ 35) that touch the helmet are at least 20 from one of the three tinted
  tones; other colours that touch the helmet are at least 20 from the tinted mid tone;
  every other colour is at least 10 from it.
- **Lambs (goslings)** are the same sheet at `lambScale` 0.6: 0.9 screen pixels per art
  pixel on a 1× display, which drops about one row and column in ten. Eyes, goggles and
  outline details are at least 2 pixels where they must survive that.
- **Light and dark wallpapers.** The outline reads on both.

## The sheep (`bellwether`)

Meant to feel like eSheep: the same genre, the same kind of character, the same kinds of
behaviour. "Similar" means genre and feel, not eSheep's frames, poses or pixels: every
frame is drawn fresh from this description.

- A small, round, white or cream **woolly sheep**, drawn side-on in pixel art, with a
  **dark face** and **dark legs**. Cheerful and bouncy.
- It wears a **flight helmet with goggles**. The helmet (shell, ear flaps, chin strap)
  carries the session colour.
- **No horns.** The helmet covers the head, and a second coloured shape would compete
  with it.

As drawn (review round 3, every animation; 53 unique frames): the body is a cream cloud
of wool about 24 × 18 pixels with scalloped curls, lit from the upper left; the head is
about 20 × 18, overlapping the front of the body, so the sheep reads as mostly head and
wool. The helmet is a round leather dome with a two-pixel light rim along the top, the
back, the brow, the ear flap and the chin strap, so even a black session's helmet has a
light contour against the dark face and a dark wallpaper. Steel goggles with pale lenses
are pushed up on the brow, their dark strap round the dome. The face is a dark charcoal
with a two-pixel white eye and a small blush; the legs are short and dark, the far ones
darker. A pale-yellow `star` colour is used only for the dizzy stars.

- **walk** (4): two contact frames with the legs spread, two passing frames with a hoof
  up and the sheep a pixel higher. **arrive** plays the same frames.
- **run** (4): a gallop: stretched out, airborne with every leg tucked, landing,
  gathering. **leave** plays the same frames, faster off the strip.
- **graze** (4): the head goes down to the grass, nibbles and blinks.
- **chew** (3): head up, eyes shut happily, the jaw working.
- **sleep** (4) and **sleep loop** (3): lying down, the head lowering, eyes closing; a
  white "z" with a thin dark outline drifts up while it breathes. The "z" is an overlay
  (`pet.json` `overlays`), so a sheep sleeping facing right still shows a "z", not an "S".
- **turn** (3): side, the head turned to you, then facing you; the alert holds that frame.
- **alert** (8; 5 of its own): facing you, a crouch, a big jump with a "!", a squashed
  landing, then turn's frames backwards to end side on.
- **trip** (5): a hoof catches, nose in the grass with the hind end up, flat out with
  crossed-out eyes, pushing up, a happy blink.
- **sit** (3): settling, then sitting on a round rump with a hind hoof tucked forward
  and the head up, blinking happily.
- **dizzy** (4): swaying, crossed-out eyes, two stars circling the helmet.
- **hop** (4): crouch, spring with the legs stretched, tuck at the top, reach down, land.
- **lie** (3): lying down awake, blinking, now and then turning to look at you.
- **carried** (3): lifted clear of the ground, eyes wide at first, legs dangling and
  kicking, swinging a pixel either way.
- **land** (3; 1 of its own): legs reaching for the ground, the crouch, standing side on.

## The goose (`brah`)

- A **white goose** with a **yellow-orange beak and feet**, a little cheeky.
- It wears the **flight helmet with goggles in every frame**.
- **The helmet never touches the beak**: at least one pixel of feather, outline or goggle
  lies between them in every frame, because no orange can be far from the orange session
  colour. The beak is a yellow-orange or ochre at least 10 (CIEDE2000) from `#FF8C00`
  (for example `#F0B060`). A goggle lens that touches the helmet needs a colour such as
  `#DDE8F0`.
- **Visibly distinct from Untitled Goose Game's goose** (a flat-shaded, unoutlined white
  goose with an orange beak and feet, a honk pose with the neck stretched and wings out,
  and a stolen bell). Ours wears the helmet and goggles in every frame, has a dark pixel
  outline, has its own proportions (a rounder, chunkier body and a shorter neck), carries
  no bell or other prop, and never uses the outstretched-neck honk as a pose.
- Its mood came from images the maintainer looked at. They are not in this repository and
  were not given to the developer: the only brief is this text.
