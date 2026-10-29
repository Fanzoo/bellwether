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
| `brah` | an aviator goose: a Canada goose in a flight helmet with goggles, a little cheeky | M10b, after 1.0 (approved 2026-10-19) |

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

Rewritten 2026-10-19 for the maintainer's decision after review round 1 (distribution
plan, deviation 141): round 1's white goose read as a duck, so the goose is a Canada
goose.

- A **Canada goose**, a little cheeky. It wears the **flight helmet with goggles in
  every frame**.
- **Head and neck:** a **long black neck** and a black head. The goose stands tall, the
  neck rising from the breast in a gentle S: that is what makes it a goose and not a duck.
  The neck is never stretched forward in a honk, and the wings never spread.
- **The white chinstrap:** a bold white patch from the cheek, under the chin, to the
  other cheek. It is the species' signature mark and stays visible below the helmet in
  the side and front views; the helmet's ear flap and chin strap are shrunk or dropped
  where they would hide it. The goggles stay on the dome.
- **Body:** a brown-grey back and wings, with pale edges on the feathers (at pixel scale,
  a lighter edge row on the wing), a paler buff or light-grey breast and belly, and a
  clean line at the base of the neck where the black meets the pale breast.
- **Tail:** black, with **white feathers under the tail** and a white band across the
  rump.
- **Bill, legs and feet are black** (dark grey for shading). The black bill and head are
  dark edge colours under (d) rule 1, as the sheep's dark face is: the helmet's light rim
  keeps them apart from a black session's helmet. The bill doesn't touch the helmet: at
  least a pixel of black head, chinstrap or feather lies between them.
- **Visibly distinct from Untitled Goose Game's goose** (a flat-shaded, unoutlined white
  goose with an orange beak and feet, a honk pose with the neck stretched and wings out,
  and a stolen bell). Ours is a brown-and-black Canada goose with a white chinstrap, not a
  white goose; it wears the helmet and goggles in every frame, has a dark pixel outline,
  carries no bell or other prop, and never uses the outstretched-neck honk as a pose.
- Its mood came from images the maintainer looked at. They are not in this repository and
  were not given to the developer: the only brief is this text, and the Canada goose's
  field marks above were written from general knowledge of the species, not from an
  image.

As drawn (review round 2, every animation; 53 unique frames; approved by the maintainer
2026-10-19): the goose stands about 39 pixels tall. A small black head under the helmet (a
round dome with a light rim, goggles pushed up on its front, and a short ear flap that
stops above the chinstrap); no chin strap, so the white chinstrap shows whole, from the
cheek down under the throat. A long black neck, lit along its front edge, rising from
the breast in a gentle S. A body about 29 × 14: a pale buff breast and belly, the
brown-grey wing with short, staggered pale feather edges, a white band across the rump,
the black tail and white under-tail feathers. Black bill, legs and feet (the far leg a
shade darker). The cheeky tell is the eye: in profile a heavy dark lid over a white eye,
a sidelong, smug look; from the front both eyes glance down to the same side, over the
white chinstrap. A pale-yellow `star` is used only for the dizzy stars; the sleeping "z"
is an overlay, as on the sheep.

- **walk** (4) and **arrive**: contact and passing frames, the goose a pixel higher when
  passing. **run** (4) and **leave**: the neck drawn in and leaning, stretched, airborne,
  landing, gathering.
- **graze** (4): the neck reaching forward and down from the breast, the bill in the
  grass; nibbling and a blink. **chew** (3): head up, eyes shut happily, the bill working.
- **sleep** (4) and **sleep loop** (3): the legs fold, the neck draws in, sitting on the
  grass with the head sunk onto the breast, eyes shut; breathing; a "z" overlay.
- **turn** (3): side, the head turned to you, then facing you; **alert** (8; 5 of its
  own): a crouch with the neck drawn in, a jump with wide eyes and a "!", a squashed
  landing, then turn backwards.
- **trip** (5): a foot catches, the bill in the grass with the tail up, flat on its belly
  with crossed-out eyes and a foot stuck out behind, pushing up, a happy blink.
- **sit** (3): settling, sitting tall with the neck up and a foot peeking in front, a
  blink. **lie** (3): lying with the head low on the breast, awake, blinking, turning to
  look at you.
- **dizzy** (4): swaying, crossed-out eyes, two stars round the helmet. **hop** (4):
  crouch, spring, tuck at the top with eyes shut in glee, reach down, land.
- **carried** (3): lifted by the helmet, the neck drawn in, eyes wide, the feet dangling
  and paddling, swinging a pixel either way. **land** (3; 1 of its own): feet reaching
  down, the crouch, standing side on.

## The zombie (`bryan`)

The maintainer asked for a cute 8-bit zombie named Bryan, wearing the helmet, bundled,
and approved both review rounds (2026-10-28).

- A **cute cartoon zombie**, small and big-headed, in chunky 8-bit pixel art. Goofy, never
  gory: no blood, no exposed bone, no wounds beyond a stitched scar.
- It wears the **flight helmet with goggles in every frame**, drawn as on the sheep (the
  round dome with a light rim, the ear flap, the chin strap), so the session colour sits
  where it does on the other pets.
- **Skin** is a pale, slightly grey green, shaded with greyer greens, never purple (the
  tinter's window): no bruises or rot in purple. A droopy eye in a dark socket, a button
  nose, a crooked mouth with one tooth, a stitched scar on the cheek.
- **Clothes:** a scruffy tan-brown bomber jacket with a cream fur collar and a torn hem,
  grey-blue trousers and scuffed brown shoes.
- **The zombie pose:** arms held straight out in front while standing and walking (sitting,
  they hang limp at his sides, the hands on the ground), a stiff shuffling walk with the
  head lolling.
- **arrive** is rising out of the ground: a hand pokes up through a heap of dirt, then the
  helmet and face, then Bryan to the waist, then standing. **leave** is meant to be the
  reverse, sinking back into a heap of dirt, but the strip drops a leaving pet only once it is
  off the strip, so for now he runs off like the sheep.

Round 1 drew walk, turn, sit and arrive; round 2 the other behaviours: 17 animations, 57
frames. From the front the eyes are bigger (one wide, one droopy) so the face reads at
strip size; the profile keeps the small droopy eye.
