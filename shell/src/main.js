// The strip's webview: draws the sheep and reports where they are. It has no network
// access (CSP connect-src is Tauri's IPC only) and calls only bellwether's own commands:
// get_pet, get_snapshot, report_hitboxes, sprite_clicked, sprite_drag (milestone 12),
// shell_log, and with --snapshot (debug) debug_snapshot. (--selftest also checks that the
// onboarding window's commands are refused to it.)

import { BUBBLE, layoutBubbles } from './player/bubbles.mjs';
import { familyExtent, freePlace, lambX } from './player/lambs.mjs';
import { displayLabels } from './player/labels.mjs';
import { tintHorns, tintWhole } from './player/horns.mjs';
import { fadeStep, frontmost, tuftPixels, tuftX } from './player/grass.mjs';
import { homeX, homesWithDrops } from './player/layout.mjs';
import { dragEvent, idleDrag } from './player/drag.mjs';
import { hoverTarget } from './player/hover.mjs';
import { assertSafePetXml, parsePetDocument, resolveAnimationMap } from './player/pet.mjs';
import { cleanOverlays, overlayPlacements } from './player/overlay.mjs';
import { Sprite, behaviourOf } from './player/sprite.mjs';
import { tileOf } from './player/stepper.mjs';

const invoke = window.__TAURI__.core.invoke;
const monitor = new URLSearchParams(location.search).get('monitor') ?? 'primary';
const canvas = document.getElementById('strip');
const ctx = canvas.getContext('2d');

const log = (line) => invoke('shell_log', { line: String(line).slice(0, 500) }).catch(() => {});
const warn = (line) => log(`warning: ${line}`);

// esheep64's animation names for each behaviour (config `pets.animationMap`).
const DEFAULT_ANIMATION_MAP = {
  walk: 'walk',
  run: 'run',
  graze: 'eat',
  sleep: ['sleep1a', 'sleep1b'],
  alert: 'boing',
  turn: 'rotate1a',
  trip: 'fall soft',
  sit: 'sync',
  dizzy: 'sleep2a',
  chew: 'eat',
  arrive: 'walk',
  leave: 'run',
  hop: 'jump',
  lie: 'sleep3a',
};

let pet; // { model, map, sheets: [canvas per palette colour], tileW, tileH }
let display; // config the shell passed: palette, scale, slotWidthPx, homeWanderPx
// actor id -> { sprite, actor, removedAt }; a lamb's entry also has parentId, place,
// colourIndex (its parent's tint) and projectLabel (its parent's project).
const sprites = new Map();
// The hover label's sprite, worked out on every frame from where the pointer was last
// seen and the shell's cursor poll (player/hover.mjs, deviation 136); pointer is null
// when it isn't known (it left, the page was hidden, the strip was hidden or toggled).
let hovered = null;
let pointer = null;
let dirty = true;
// Milestone 7. muted: the brain's do-not-disturb (the tray's "Quiet (alerts only)",
// `bellwether mute`). view: the shell's part of each snapshot: hidden (sharing, or the tray's
// "Hide everything (for screen sharing)"),
// showLabels (tray), near (sprite keys within fade.radiusPx of the cursor), over (this
// strip's sprite keys whose box the cursor is on, by the shell's poll).
let muted = false;
let view = { rev: -1, hidden: false, showLabels: false, near: new Set(), over: new Set(), dock: null, carry: null };

// Dragging a sheep (D10, milestone 12). gesture: the press, click or drag state machine
// (player/drag.mjs); pressId: the pressed sprite's id as the shell knows it. carried: the
// sheep this strip is carrying {key, id, grabDx, fromX, dir}. guest: a sheep carried here
// from another monitor's strip (the shell's `carry`), drawn here until it is let go;
// lastGuest: where it was when the carry ended, for its fall if it was dropped here.
let gesture = idleDrag();
let pressId = null;
let carried = null;
let guest = null;
let lastGuest = null;
let lastCancel = null;
let lastPlacement = null;
// Until the first snapshot is drawn: a sheep dropped here before a reload doesn't land again.
let booted = false;

/** Whether the strip animates at all: not while muted or hidden. */
const quiet = () => muted || view.hidden;

// ---------------------------------------------------------------- the pet

async function loadPet(onMissing = warn) {
  const p = await invoke('get_pet');
  display = p.display;
  const doc = new DOMParser().parseFromString(assertSafePetXml(p.xml), 'application/xml');
  if (doc.getElementsByTagName('parsererror').length) throw new Error(`data/pets/${p.folder}/animations.xml is not valid XML`);
  const model = parsePetDocument(doc);
  // config.json's map wins; the defaults cover keys an older config.json lacks (hop).
  const map = resolveAnimationMap(model, { ...DEFAULT_ANIMATION_MAP, ...(p.animationMap ?? {}) }, onMissing);
  if (!map.walk) throw new Error(`pet ${p.folder}: animationMap has no usable "walk" animation`);
  const image = new Image();
  image.src = `data:image/png;base64,${model.png}`;
  await image.decode();
  const tileW = image.width / model.tilesx;
  const tileH = image.height / model.tilesy;
  const base = keyed(image, model.transparency);
  // Every colour is a tint, the first included (Milestone 10: no untinted sheep).
  const sheets = display.palette.map((hex) => tinted(base, hex, display.tint));
  // A bundled pet's never-mirrored overlays (the sleeping "z"); an imported pet has none.
  const overlays = cleanOverlays(p.overlays, model.tilesx * model.tilesy);
  pet = { model, map, sheets, tileW, tileH, overlays, ...grassFor(model, map, base, tileW, tileH) };
  log(`pet ${p.folder} loaded: ${Object.keys(model.animations).length} animations, ${tileW}x${tileH} tiles, behaviours ${Object.keys(map).join(' ')}`);
}

/**
 * Grass, made once at load: the tuft at one pixel per art pixel (drawn scaled like the
 * sprite, so it shares its pixel grid), facing both ways, and how far forward a grazing
 * head reaches (the frontmost column over the graze and chew frames).
 */
function grassFor(model, map, base, tileW, tileH) {
  const g = base.getContext('2d');
  const sheet = g.getImageData(0, 0, base.width, base.height).data;
  const frames = [...new Set([...(map.graze ?? []), ...(map.chew ?? [])].flatMap((id) => model.animations[id]?.frames ?? []))];
  const alphaAt = (f, x, y) => {
    const { col, row } = tileOf(model, f);
    return sheet[((row * tileH + y) * base.width + col * tileW + x) * 4 + 3];
  };
  const canvasOf = (flip) => {
    const t = tuftPixels(1, flip);
    const c = document.createElement('canvas');
    c.width = t.w;
    c.height = t.h;
    c.getContext('2d').putImageData(new ImageData(t.data, t.w, t.h), 0, 0);
    return c;
  };
  return { grazeFront: frontmost(frames, tileW, tileH, alphaAt), tuft: [canvasOf(false), canvasOf(true)] };
}

/** Each sprite's tuft: fade it in while grazing or chewing, out a moment after. */
function stepGrass(entry, now) {
  const { sprite } = entry;
  const eating = !sprite.gone && !sprite.held && !sprite.fall && entry.removedAt === undefined && !sprite.oneShot && (sprite.behaviour === 'graze' || sprite.behaviour === 'chew');
  const before = entry.grass?.alpha ?? 0;
  entry.grass = fadeStep(entry.grass, { eating, now });
  if (eating) {
    const d = sprite.drawInfo();
    const s = sprite.width / pet.tileW;
    const tuftW = pet.tuft[0].width * s;
    entry.grassAt = { x: tuftX({ x: d.x, width: sprite.width, tileW: pet.tileW, front: pet.grazeFront, flip: d.flip, tuftW }), flip: d.flip, s };
  }
  return entry.grass.alpha !== before;
}

function drawGrass(entry) {
  const g = entry.grass;
  if (!g || g.alpha <= 0 || !entry.grassAt) return;
  const { x, flip, s } = entry.grassAt;
  const t = pet.tuft[flip ? 1 : 0];
  ctx.save();
  ctx.globalAlpha = g.alpha * fadeOf(entry.key);
  ctx.drawImage(t, x, stripSize().height - t.height * s, t.width * s, t.height * s);
  ctx.restore();
}

/** The sheet with its key colour (magenta in esheep's art) made transparent. */
function keyed(image, transparency) {
  const c = document.createElement('canvas');
  c.width = image.width;
  c.height = image.height;
  const g = c.getContext('2d');
  g.drawImage(image, 0, 0);
  if (/magenta/i.test(transparency)) {
    const img = g.getImageData(0, 0, c.width, c.height);
    const d = img.data;
    for (let i = 0; i < d.length; i += 4) {
      if (d[i] > 240 && d[i + 1] < 16 && d[i + 2] > 240) d[i + 3] = 0;
    }
    g.putImageData(img, 0, 0);
  }
  return c;
}

/**
 * One sheet per palette colour, made once when the pet loads (never per frame).
 * `pets.tint` "horns" (the default): only the horns take the colour, keeping their
 * shading (player/horns.mjs). "whole": the old multiply tint over the whole sheep.
 */
function tinted(base, hex, mode = 'horns') {
  if (!/^#?[0-9a-f]{6}$/i.test(hex ?? '')) return base;
  const c = document.createElement('canvas');
  c.width = base.width;
  c.height = base.height;
  const x = c.getContext('2d');
  x.drawImage(base, 0, 0);
  const img = x.getImageData(0, 0, c.width, c.height);
  if (mode === 'whole') tintWhole(img.data, hex);
  else tintHorns(img.data, hex);
  x.putImageData(img, 0, 0);
  return c;
}

// ------------------------------------------------------------ the flock

function stripSize() {
  return { width: window.innerWidth, height: window.innerHeight };
}

function spriteSize() {
  return { w: Math.round(pet.tileW * display.scale), h: Math.round(pet.tileH * display.scale) };
}

/** Lambs: pets.lambScale of a sheep (0.6 by default), kept between 0.3 and 1. */
function lambSize() {
  const k = Math.min(1, Math.max(0.3, display.lambScale ?? 0.6));
  return { w: Math.round(pet.tileW * display.scale * k), h: Math.round(pet.tileH * display.scale * k) };
}

// A lamb wanders this far either side of its place beside the parent.
const LAMB_WANDER = 6;

function home(slot) {
  const { w } = spriteSize();
  return homeX(slot, { stripWidth: stripSize().width, slotWidth: display.slotWidthPx, spriteWidth: w });
}

/** The shell's part: hidden, labels, the fade. A fade change is drawn at once (≤ 200 ms). */
function applyView(v) {
  const near = new Set(Array.isArray(v.near) ? v.near : []);
  const nearChanged = near.size !== view.near.size || [...near].some((id) => !view.near.has(id));
  if (!!v.hidden !== view.hidden) log(`strip ${v.hidden ? 'hidden: drawing stops' : 'shown: drawing resumes'}`);
  const dock = v.dock && Number.isFinite(v.dock.lo) && Number.isFinite(v.dock.hi) && v.dock.hi > v.dock.lo ? { lo: v.dock.lo, hi: v.dock.hi } : null;
  const dockChanged = JSON.stringify(dock) !== JSON.stringify(view.dock ?? null);
  // Hide everything or Show labels toggled: whatever the pointer was on, it is forgotten
  // until it moves again (the tray's menu is where the cursor is now).
  if (!!v.hidden !== view.hidden || !!v.showLabels !== view.showLabels) pointer = null;
  const over = new Set(Array.isArray(v.over) ? v.over : []);
  const carry = v.carry && typeof v.carry === 'object' ? v.carry : null;
  view = { rev: v.rev, hidden: !!v.hidden, showLabels: !!v.showLabels, near, over, dock, carry };
  // A drag ends when the strip is hidden, or when the shell ended it (its fail-safe, the
  // ceiling, a display change): the sheep is set down where it was picked up.
  if (view.hidden) cancelGesture('the strip is hidden');
  if (v.dragCancel && v.dragCancel.n !== lastCancel?.n) {
    lastCancel = v.dragCancel;
    if (pressId === v.dragCancel.id) cancelGesture('the shell ended it', false);
  }
  applyCarry(carry, performance.now());
  updateHover();
  if (dockChanged && pet) placeFamilies();
  dirty = true;
  if (nearChanged && !quiet()) {
    const now = performance.now();
    lastDraw = now;
    draw(now);
    lastSig = signature(now);
    dirty = false;
    if (display.trace) log(`trace: fade drawn: ${near.size} sprite(s) near the cursor`);
  }
}

function apply(snapshot) {
  const now = performance.now();
  if (!!snapshot.muted !== muted) {
    muted = !!snapshot.muted;
    pointer = null;
    log(muted ? 'muted: animation stops; only a sheep that needs you shows a badge' : 'unmuted');
    if (muted) cancelGesture('Quiet is on');
  }
  const seen = new Set();
  let urgent = false;
  // Same-project sheep get a short distinguisher ("my-project · p4").
  const labels = displayLabels(snapshot.flock.map((a) => ({ id: a.id, label: a.label, terminal: a.terminal, slot: a.slot })));
  for (const actor of snapshot.flock) {
    seen.add(actor.id);
    let entry = sprites.get(actor.id);
    if (!entry) {
      const { w, h } = spriteSize();
      const { width, height } = stripSize();
      const sprite = new Sprite({ model: pet.model, map: pet.map, width: w, height: h, stripWidth: width, stripHeight: height, home: home(actor.slot), wander: display.homeWanderPx, warn });
      entry = { key: actor.id, sprite, actor, fresh: true };
      sprites.set(actor.id, entry);
      if (actor.dropped) {
        // Dropped here from another strip (D10): it starts where it was let go, not at its
        // home and without arriving, and falls from where it was held.
        sprite.x = sprite.clampToStrip(actor.dropped.centreX - w / 2);
        entry.fresh = false;
        const held = lastGuest?.id === actor.id && now - lastGuest.at < 3000 ? lastGuest : null;
        if (held) entry.dropDir = held.dir;
        if (booted) {
          sprite.hold(now);
          sprite.carry(sprite.x, held ? held.top : 0);
        } else sprite.landedSeq = actor.dropped.seq;
        log(`sheep ${actor.id} (${actor.label}) dropped here at x ${Math.round(actor.dropped.centreX)}, colour ${actor.colourIndex}`);
      } else log(`sheep ${actor.id} (${actor.label}) arrives in slot ${actor.slot}, colour ${actor.colourIndex}`);
    }
    entry.actor = actor;
    entry.removedAt = undefined;
    if (update(entry, actor, now)) urgent = true;
    if (actor.dropped && actor.dropped.seq !== entry.sprite.landedSeq) {
      if (entry.awaitingDrop) {
        // Dropped from here, already landing: this is its record, not a second landing.
        entry.sprite.landedSeq = actor.dropped.seq;
        entry.awaitingDrop = false;
      } else if (entry.sprite.held && entry !== carriedEntry()) {
        if (entry.sprite.setDown(now, { seq: actor.dropped.seq })) urgent = true;
      } else entry.sprite.landedSeq = actor.dropped.seq;
    }
    if (entry.localDrop && actor.dropped?.seq === entry.localDrop.seq) entry.localDrop = null;
    entry.shownLabel = labels.get(actor.id) ?? actor.label;
    for (const lamb of actor.lambs ?? []) {
      // Keyed under the parent: agent ids are unique per session, not across them
      // (two replays of one fixture share them).
      const key = lambKey(actor.id, lamb.id);
      seen.add(key);
      const le = sprites.get(key) ?? addLamb(lamb, actor.id, entry);
      Object.assign(le, { actor: lamb, parentId: actor.id, colourIndex: actor.colourIndex, projectLabel: entry.shownLabel, removedAt: undefined });
      if (update(le, lamb, now)) urgent = true;
    }
  }
  for (const [id, entry] of sprites) {
    if (seen.has(id) || entry.removedAt !== undefined) continue;
    if (entry.carriedAway) {
      // Carried to another monitor's strip and dropped there: gone from here at once,
      // without running off. Its lambs leave as on any move to another monitor.
      sprites.delete(id);
      log(`sheep ${id} carried to another monitor`);
      continue;
    }
    // Gone from the brain's flock: a sheep runs off, a lamb trots back into its
    // parent; either is dropped once it's gone.
    entry.removedAt = now;
    log(`${entry.parentId ? 'lamb' : 'sheep'} ${id} -> leave (gone from the flock)`);
    entry.sprite.setBehaviour('leave', now);
  }
  // The sprite being pressed or carried is leaving or gone: the drag ends at once (it was
  // set down where it is and runs off, as `leave` does).
  if (gesture.phase !== 'idle') {
    const e = sprites.get(gesture.id);
    if (!e || e.removedAt !== undefined || e.actor.state === 'leaving') cancelGesture('its session ended');
  }
  placeFamilies();
  dirty = true;
  if (!booted) {
    booted = true;
    // M12 step 1 (e): how long a new strip takes to draw its first snapshot.
    log(`first snapshot drawn: ${snapshot.flock.length} sheep, ${Math.round(now)} ms after the page started`);
  }
  if (urgent) {
    lastDraw = now;
    draw(now);
    lastSig = signature(now);
    dirty = false;
  }
}

/**
 * Each sheep's home and how far it may wander, from the room its family (it, its
 * lambs and any "+N") takes up, so families never interleave (layout.mjs). Called when
 * a snapshot arrives and when a lamb's place frees up. A new sheep starts at its home;
 * the others walk there when it moves.
 */
function placeFamilies() {
  const lambW = lambSize().w;
  const parents = [...sprites.values()].filter((e) => !e.parentId && e.removedAt === undefined);
  const families = parents.map((e) => {
    const places = [...sprites.values()].filter((l) => l.parentId === e.actor.id).map((l) => l.place + 1);
    const used = Math.max(0, ...places) + (e.actor.hiddenLambCount > 0 ? 1 : 0);
    return { id: e.actor.id, slot: e.actor.slot, extent: familyExtent(used, { parentW: e.sprite.width, lambW }), drop: dropOf(e) };
  });
  // On the Dock's top edge when its extent is known (Accessibility granted); else the full
  // strip. A dropped sheep lives around its drop, clear of the others (D10).
  const homes = homesWithDrops(families, { stripWidth: stripSize().width, slotWidth: display.slotWidthPx, wanderMax: display.homeWanderPx, dock: view.dock });
  for (const e of parents) {
    const h = homes.get(e.actor.id);
    if (!h) continue;
    e.sprite.setHome(h.centre - e.sprite.width / 2);
    e.sprite.wander = h.wander;
    e.inFront = !!h.noRoom;
    const drop = e.actor.dropped ?? e.localDrop;
    if (h.noRoom && drop && e.noRoomSeq !== drop.seq) {
      e.noRoomSeq = drop.seq;
      log(`sheep ${e.actor.id} dropped on ${monitor} at x ${Math.round(drop.centreX)}: no room, so it stays there, in front`);
    }
    if (e.fresh) {
      e.sprite.x = e.sprite.clampToStrip(e.sprite.home);
      e.fresh = false;
    }
  }
}

/** A sheep's drop for the layout: the shell's record, or this strip's own until it arrives. */
function dropOf(e) {
  const d = e.actor.dropped ?? e.localDrop;
  if (!d || !Number.isFinite(d.centreX)) return null;
  return { centre: d.centreX, seq: d.seq ?? Number.MAX_SAFE_INTEGER, ghost: !!d.ghost, dir: e.dropDir ?? 1 };
}

/** Applies one actor's state to its sprite. True if a new or newly confirmed alert needs drawing now. */
function update(entry, actor, now) {
  let urgent = false;
  // A sheep dropped onto a strip doesn't arrive there: it was carried in (D10).
  const behaviour = actor.dropped && behaviourOf(actor) === 'arrive' ? 'walk' : behaviourOf(actor);
  const what = entry.parentId ? 'lamb' : 'sheep';
  if (behaviour !== entry.sprite.behaviour) log(`${what} ${actor.id} -> ${behaviour} (${actor.state}${actor.flavour ? ` ${actor.flavour}` : ''}${actor.alert ? ', alert' : ''})`);
  // A new or newly confirmed alert is drawn at once, not on the next 150 ms step.
  const alertKey = actor.alert ? `${actor.alert.kind}${actor.alert.confirmed ? '+confirmed' : ''}` : '';
  if (alertKey !== (entry.alertKey ?? '')) {
    if (actor.alert && (!entry.alertKey || actor.alert.confirmed)) urgent = true;
    entry.alertKey = alertKey;
  }
  entry.sprite.setBehaviour(behaviour, now);
  entry.sprite.setConfirmed(!!actor.alert?.confirmed, now);
  if (actor.effect && actor.effect.at !== entry.effectAt) {
    entry.effectAt = actor.effect.at;
    if (actor.effect.name === 'trip' && !actor.alert) entry.sprite.playOnce('trip', now);
  }
  return urgent;
}

const lambKey = (parentId, lambId) => `${parentId}/${lambId}`;

/**
 * A new lamb: it starts inside its parent and trots out to the first free place
 * beside it (lambs.mjs). When it leaves, it trots back into the parent.
 */
function addLamb(lamb, parentId, parent) {
  const { w, h } = lambSize();
  const { width, height } = stripSize();
  const taken = new Set([...sprites.values()].filter((e) => e.parentId === parentId && e.removedAt === undefined).map((e) => e.place));
  const place = freePlace(taken);
  const ps = parent.sprite;
  const sprite = new Sprite({ model: pet.model, map: pet.map, width: w, height: h, stripWidth: width, stripHeight: height, home: lambX(place, { parentX: ps.x, parentW: ps.width, lambW: w, stripWidth: width }), wander: LAMB_WANDER, warn });
  sprite.x = ps.x + ps.width / 2 - w / 2;
  sprite.returnTo = () => {
    const p = sprites.get(parentId);
    if (!p || p.removedAt !== undefined || p.sprite.gone) return null; // the parent is going too: run off
    return p.sprite.x + p.sprite.width / 2 - w / 2;
  };
  const entry = { key: lambKey(parentId, lamb.id), sprite, actor: lamb, parentId, place };
  sprites.set(lambKey(parentId, lamb.id), entry);
  log(`lamb ${lamb.id} (${lamb.label}) arrives beside ${parentId}, place ${place}`);
  return entry;
}

async function follow() {
  let rev = -1;
  for (;;) {
    try {
      if (display.trace) log(`trace: get_snapshot after ${rev}`);
      const snap = await invoke('get_snapshot', { after: rev, shellAfter: view.rev });
      if (display.trace) log(`trace: got rev ${snap?.rev}`);
      if (snap && typeof snap.rev === 'number') {
        // A drop or a sheep moving monitor changes this strip's flock with no new brain
        // snapshot: the shell's `placement` says so.
        const placement = snap.shell?.placement;
        const moved = placement !== undefined && placement !== lastPlacement;
        if (snap.shell && snap.shell.rev !== view.rev) applyView(snap.shell);
        if (snap.rev !== rev || moved) apply(snap);
        if (placement !== undefined) lastPlacement = placement;
        rev = snap.rev;
      } else {
        await sleep(1000);
      }
    } catch (e) {
      warn(`get_snapshot failed: ${e}`);
      await sleep(2000);
    }
  }
}

// --------------------------------------------------------------- drawing

function resize() {
  const ratio = window.devicePixelRatio || 1;
  const { width, height } = stripSize();
  canvas.width = Math.round(width * ratio);
  canvas.height = Math.round(height * ratio);
  ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
  ctx.imageSmoothingEnabled = false;
  for (const { sprite } of sprites.values()) {
    sprite.stripWidth = width;
    sprite.stripHeight = height;
  }
  if (pet && display) placeFamilies();
  dirty = true;
}

function draw(now = performance.now()) {
  const { width, height } = stripSize();
  ctx.clearRect(0, 0, width, height);
  if (view.hidden) return;
  if (muted) {
    drawMuted();
    return;
  }
  // Sheep first (one dropped where there was no room in front of the others), then lambs
  // in front of them. The carried sheep is drawn last, below.
  const all = [...sprites.values()].filter((e) => !e.carriedAway && e.key !== carried?.key);
  for (const e of [...all.filter((e) => !e.parentId && !e.inFront), ...all.filter((e) => !e.parentId && e.inFront), ...all.filter((e) => e.parentId)]) drawSprite(e, now);
  // The grass in front of the mouths, so a grazing sheep is eating into it.
  for (const e of all) drawGrass(e);
  for (const e of all) if (!e.parentId && e.removedAt === undefined && e.actor.hiddenLambCount > 0) moreLambs(e);
  const bubbled = drawBubbles(now);
  if (view.showLabels) {
    // The tray's "Show labels": every sheep without a bubble shows its project.
    for (const [id, e] of sprites) {
      if (e.parentId || e.removedAt !== undefined || e.sprite.gone || bubbled.has(id) || id === hovered || e.carriedAway || id === carried?.key) continue;
      const d = e.sprite.drawInfo(now);
      label(e.shownLabel ?? e.actor.label, d.x + e.sprite.width / 2, d.y - 6, fadeOf(id));
    }
  }
  const h = hovered && sprites.get(hovered);
  if (h && !bubbled.has(hovered) && !h.sprite.gone && h.removedAt === undefined) {
    const { sprite, actor, shownLabel, parentId } = h;
    // Where its sheep is now: the label follows it.
    const d = sprite.drawInfo(now);
    // A sheep shows its project (told apart from same-project sheep); a lamb its agent type.
    label(parentId ? actor.label : shownLabel ?? actor.label, d.x + sprite.width / 2, d.y - 6);
  }
  // Last, above bubbles, labels and lambs: the carried sheep, at full opacity (D10).
  drawCarried(now);
}

/** The entry this strip is carrying, if it is drawn here (not on another monitor's strip). */
function carriedEntry() {
  const e = carried && sprites.get(carried.key);
  if (!e) return null;
  return view.carry?.away && view.carry.id === e.actor.id ? null : e;
}

/**
 * The carried sheep, here or carried in from another strip: drawn last, never faded, with
 * no label. An alerting one wears the "!" badge instead of its bubble; a parent whose
 * lamb waits still wears its "lamb" tag.
 */
function drawCarried(now) {
  const e = carriedEntry();
  if (e) {
    drawSprite(e, now, { full: true });
    const a = e.actor;
    const d = e.sprite.drawInfo(now);
    if (a.alert) badge(d.x + e.sprite.width - 6, d.y + 4, a.alert.kind);
    else if (a.lambAlert) {
      const waiting = (a.lambs ?? []).filter((l) => l.alert);
      const onStrip = waiting.find((l) => sprites.get(lambKey(e.key, l.id)) && !sprites.get(lambKey(e.key, l.id)).sprite.gone);
      if (onStrip) lambTag(e, onStrip.alert.kind, !!onStrip.alert.confirmed, now);
      else badge(d.x + e.sprite.width - 6, d.y + 4, 'lamb');
    }
  }
  if (guest) {
    drawSprite(guest, now, { full: true });
    if (guest.actor.alert) {
      const d = guest.sprite.drawInfo(now);
      badge(d.x + guest.sprite.width - 6, d.y + 4, guest.actor.alert.kind);
    }
  }
}

/** The cursor-near fade: fade.opacity for a sprite (and its bubble) near the cursor. */
function fadeOf(id) {
  return view.near.has(id) ? display.fadeOpacity ?? 0.25 : 1;
}

/**
 * Muted (do-not-disturb): no sheep, no animation; only a small static "!" where a
 * sheep or lamb needs you.
 */
function drawMuted() {
  for (const [, e] of sprites) {
    if (e.removedAt !== undefined || e.sprite.gone) continue;
    // On whoever is waiting: a lamb's prompt badges the lamb, not its parent too,
    // unless that lamb isn't on the strip.
    const a = e.actor;
    const lambOnStrip = (a.lambs ?? []).some((l) => l.alert && sprites.has(lambKey(e.key, l.id)));
    const kind = a.alert?.kind ?? (!e.parentId && a.lambAlert && !lambOnStrip ? 'lamb' : null);
    if (!kind) continue;
    const d = e.sprite.drawInfo();
    badge(d.x + e.sprite.width / 2, stripSize().height - 12, kind);
  }
}

function drawSprite({ key, sprite, actor, colourIndex }, now, { full = false } = {}) {
  if (sprite.gone) return; // a lamb back inside its parent, or a sheep off the strip
  const d = sprite.drawInfo(now);
  // The carried sheep is exempt from the cursor-near fade (full).
  if (!full) d.opacity *= fadeOf(key);
  const { width: w, height: h } = sprite;
  const sheet = pet.sheets[colourIndex ?? actor.colourIndex] ?? pet.sheets[0];
  const { col, row } = tileOf(pet.model, d.frame);
  ctx.save();
  ctx.globalAlpha = d.opacity;
  if (d.flip) {
    ctx.translate(d.x + w, d.y);
    ctx.scale(-1, 1);
    ctx.drawImage(sheet, col * pet.tileW, row * pet.tileH, pet.tileW, pet.tileH, 0, 0, w, h);
  } else {
    ctx.drawImage(sheet, col * pet.tileW, row * pet.tileH, pet.tileW, pet.tileH, d.x, d.y, w, h);
  }
  ctx.restore();
  // Overlays after the restore: placed for the facing, but never mirrored.
  for (const o of overlayPlacements(pet.overlays, d.frame, { x: d.x, y: d.y, width: w, tileW: pet.tileW, flip: d.flip })) {
    const t = tileOf(pet.model, o.tile);
    ctx.save();
    ctx.globalAlpha = d.opacity;
    ctx.drawImage(sheet, t.col * pet.tileW, t.row * pet.tileH, o.sw, o.sh, o.dx, o.dy, o.dw, o.dh);
    ctx.restore();
  }
}

/** "+N": lambs beyond behaviour.maxLambsShown, in the next free place beside the parent. */
function moreLambs(parent) {
  const { w, h } = lambSize();
  const { width, height } = stripSize();
  const taken = new Set([...sprites.values()].filter((e) => e.parentId === parent.actor.id && e.removedAt === undefined).map((e) => e.place));
  const x = lambX(freePlace(taken), { parentX: parent.sprite.x, parentW: parent.sprite.width, lambW: w, stripWidth: width });
  const text = `+${parent.actor.hiddenLambCount}`;
  ctx.save();
  ctx.font = '600 11px -apple-system, system-ui, sans-serif';
  const pw = ctx.measureText(text).width + 10;
  const px = Math.min(Math.max(1, x + w / 2 - pw / 2), width - pw - 1);
  const py = height - h / 2 - 8;
  ctx.fillStyle = 'rgba(30, 30, 30, 0.75)';
  ctx.beginPath();
  ctx.roundRect(px, py, pw, 16, 8);
  ctx.fill();
  ctx.fillStyle = '#fff';
  ctx.textBaseline = 'middle';
  ctx.fillText(text, px + 5, py + 8.5);
  ctx.restore();
}

/**
 * A parent's small "a lamb needs you" tag, on its top-left, while its lamb shows its
 * own bubble. It stays within the parent's own width, so it never lies over a neighbour.
 */
function lambTag(parent, kind, confirmed, now) {
  const d = parent.sprite.drawInfo(now);
  const accent = ACCENT[kind] ?? ACCENT.permission;
  ctx.save();
  ctx.font = '700 10px -apple-system, system-ui, sans-serif';
  const tw = ctx.measureText('lamb').width + 10;
  const x = Math.max(1, d.x + 2);
  const y = d.y - 4;
  ctx.fillStyle = 'rgba(255, 255, 255, 0.96)';
  ctx.strokeStyle = accent;
  ctx.lineWidth = confirmed ? 2 : 1.25;
  ctx.setLineDash([3, 2]);
  ctx.beginPath();
  ctx.roundRect(x, y, tw, 14, 7);
  ctx.fill();
  ctx.stroke();
  ctx.fillStyle = accent;
  ctx.textBaseline = 'middle';
  ctx.fillText('lamb', x + 5, y + 7.5);
  ctx.restore();
}

// ---------------------------------------------------------------- bubbles

const FONT = '12px -apple-system, system-ui, sans-serif';
const FONT_BOLD = '600 12px -apple-system, system-ui, sans-serif';
const FONT_SUB = '11px -apple-system, system-ui, sans-serif';
const ACCENT = { permission: '#d4880f', elicitation: '#3b6fd0', input: '#7b4fd0', lamb: '#d4880f' };

/** Text widths for the layout, from the canvas itself. */
function measure(text, bold) {
  ctx.font = bold ? FONT_BOLD : FONT;
  return ctx.measureText(text).width;
}
function measureSub(text) {
  ctx.font = FONT_SUB;
  return ctx.measureText(text).width;
}

/**
 * Draws a bubble over every alerting sheep and lamb, laid out so neighbours don't
 * overlap. A lamb's bubble reads "<agent type>: <question>", with its parent's
 * project as the label, and its parent wears a small "lamb" tag. Only when the
 * alerting lamb isn't on the strip does the parent carry the question itself, in a
 * dashed bubble. All text comes from the brain's snapshot, already redacted and
 * truncated; it is drawn with fillText on the canvas, never as HTML. Returns the
 * ids that got a bubble.
 */
function drawBubbles(now) {
  const { height } = stripSize();
  const sheepTop = height - spriteSize().h;
  const items = [];
  const tags = [];
  // The carried sheep has no bubble: it wears a badge (drawCarried), logged as shown here.
  const badged = new Map();
  const held = carried && sprites.get(carried.key);
  if (held?.actor.alert) badged.set(held.key, `${held.actor.alert.kind}${held.actor.alert.confirmed ? '+confirmed' : ''}`);
  for (const [id, entry] of sprites) {
    const { sprite, actor } = entry;
    if (sprite.gone || actor.state === 'leaving' || entry.removedAt !== undefined || entry === held || entry.carriedAway) continue;
    const rest = sprite.drawInfo();
    const cx = rest.x + sprite.width / 2;
    if (entry.parentId) {
      if (!actor.alert) continue;
      // On the sheep's rows (so the layout's rows hold), with a longer tail down to the lamb.
      const priority = actor.alert.confirmed ? 3 : 2;
      items.push({ id, cx, top: sheepTop, anchorTop: rest.y, text: `${actor.label}: ${actor.alert.text}`, sub: entry.projectLabel, priority, kind: actor.alert.kind, confirmed: priority === 3 });
      continue;
    }
    if (actor.alert) {
      const priority = actor.alert.confirmed ? 3 : 2;
      items.push({ id, cx, top: rest.y, text: actor.alert.text, sub: entry.shownLabel ?? actor.label, priority, kind: actor.alert.kind, confirmed: priority === 3 });
    }
    if (actor.lambAlert) {
      const waiting = (actor.lambs ?? []).filter((l) => l.alert);
      const onStrip = waiting.find((l) => sprites.get(lambKey(id, l.id)) && !sprites.get(lambKey(id, l.id)).sprite.gone);
      if (onStrip) tags.push({ entry, kind: onStrip.alert.kind, confirmed: !!onStrip.alert.confirmed });
      else if (!actor.alert) {
        const lamb = waiting[0];
        const priority = lamb?.alert?.confirmed ? 1.5 : 1;
        items.push({ id, cx, top: rest.y, text: lamb ? `${lamb.label}: ${lamb.alert.text}` : 'a lamb needs you', sub: entry.shownLabel ?? actor.label, priority, kind: 'lamb', confirmed: priority === 1.5 });
      }
    }
  }
  for (const t of tags) lambTag(t.entry, t.kind, t.confirmed, now);
  const bubbled = new Set();
  if (!items.length) {
    logDrawnAlerts(badged);
    return bubbled;
  }
  const { width } = stripSize();
  const { placed, dropped } = layoutBubbles(items, { stripWidth: width, stripHeight: height, measure, measureSub });
  const byId = new Map(items.map((i) => [i.id, i]));
  // The upper row first: a lower bubble then covers the upper row's tails where they
  // pass behind it, instead of a tail crossing its text.
  for (const b of [...placed].sort((p, q) => q.row - p.row)) {
    const item = byId.get(b.id);
    const lift = sprites.get(b.id).sprite.bounce(now);
    bubble(b, item, lift);
    bubbled.add(b.id);
  }
  for (const id of dropped) {
    // No room for a bubble: a "!" badge on the sheep (or lamb) says it is waiting.
    const { sprite } = sprites.get(id);
    const d = sprite.drawInfo(now);
    badge(d.x + sprite.width - 6, d.y + 4, byId.get(id).kind);
    bubbled.add(id);
  }
  if (dropped.length) maybeWarnDropped(dropped.length);
  logDrawnAlerts(new Map([...badged, ...items.map((i) => [i.id, `${i.kind}${i.confirmed ? '+confirmed' : ''}`])]));
  return bubbled;
}

/**
 * Alert timing lines (Milestone 5's 300 ms check), written when the bubble or badge
 * is drawn, not when the snapshot arrives, so they time what reaches the screen (bar
 * the compositor's next frame). Kind and state only: bubble text never goes into a log.
 */
function logDrawnAlerts(drawn) {
  for (const [id, entry] of sprites) {
    const was = entry.drawnAlert ?? '';
    const is = drawn.get(id) ?? '';
    if (was === is) continue;
    const kind = is.replace('+confirmed', '');
    if (!is) log(`alert cleared: ${id}`);
    else if (!was || was.replace('+confirmed', '') !== kind) log(`alert shown: ${id} ${kind}`);
    else if (is.endsWith('+confirmed')) log(`alert confirmed: ${id} ${kind}`);
    entry.drawnAlert = is;
  }
}

function bubble(b, item, lift) {
  const accent = ACCENT[item.kind] ?? ACCENT.permission;
  ctx.globalAlpha = fadeOf(item.id);
  const y = b.y - Math.round(lift / 2);
  const tailTop = y + b.h;
  ctx.save();
  ctx.lineWidth = item.confirmed ? 2.5 : 1.25;
  ctx.strokeStyle = accent;
  ctx.fillStyle = 'rgba(255, 255, 255, 0.96)';
  if (item.kind === 'lamb') ctx.setLineDash([4, 3]);
  ctx.beginPath();
  ctx.roundRect(b.x, y, b.w, b.h, 7);
  // A lamb's bubble sits on the sheep's rows: its tail reaches on down to the lamb.
  const reach = BUBBLE.tail + Math.max(0, (item.anchorTop ?? item.top) - item.top);
  ctx.moveTo(b.tailX - 6, tailTop);
  ctx.lineTo(b.tailX, tailTop + reach);
  ctx.lineTo(b.tailX + 6, tailTop);
  ctx.fill();
  ctx.stroke();
  // Cover the stroke where the tail joins the box.
  ctx.setLineDash([]);
  ctx.fillRect(b.tailX - 5, tailTop - 1.5, 10, 2.5);
  ctx.fillStyle = '#1d1d1f';
  ctx.textBaseline = 'middle';
  ctx.font = item.confirmed ? '700 12px -apple-system, system-ui, sans-serif' : FONT_BOLD;
  const mid = y + BUBBLE.padY + BUBBLE.lineHeight / 2;
  ctx.fillText(b.text, b.x + BUBBLE.padX, mid);
  if (b.sub) {
    // The project label, on the same line in grey: which session is asking.
    ctx.font = FONT_SUB;
    ctx.fillStyle = '#6e6e73';
    ctx.fillText(b.sub, b.subX, mid);
  }
  ctx.restore();
  ctx.globalAlpha = 1;
}

function badge(cx, cy, kind) {
  ctx.save();
  ctx.fillStyle = ACCENT[kind] ?? ACCENT.permission;
  ctx.beginPath();
  ctx.arc(cx, cy, 8, 0, Math.PI * 2);
  ctx.fill();
  ctx.fillStyle = '#fff';
  ctx.font = '700 12px -apple-system, system-ui, sans-serif';
  ctx.textAlign = 'center';
  ctx.textBaseline = 'middle';
  ctx.fillText('!', cx, cy + 0.5);
  ctx.restore();
}

let lastDropWarn = 0;
function maybeWarnDropped(n) {
  const t = performance.now();
  if (t - lastDropWarn < 30_000) return;
  lastDropWarn = t;
  log(`${n} alert bubble(s) had no room on the strip; those sheep show a "!" badge`);
}

function label(text, cx, bottom, alpha = 1) {
  ctx.save();
  ctx.globalAlpha = alpha;
  ctx.font = '12px -apple-system, system-ui, sans-serif';
  const width = ctx.measureText(text).width + 12;
  const x = Math.max(2, Math.min(cx - width / 2, stripSize().width - width - 2));
  const y = bottom - 18;
  ctx.fillStyle = 'rgba(30, 30, 30, 0.82)';
  ctx.beginPath();
  ctx.roundRect(x, y, width, 18, 6);
  ctx.fill();
  ctx.fillStyle = '#fff';
  ctx.textBaseline = 'middle';
  ctx.fillText(text, x + 6, y + 9);
  ctx.restore();
}

// -------------------------------------------------------- hitboxes, clicks

let lastBoxes = '';
let lastBoxesAt = 0;

function reportHitboxes(now, force = false) {
  // Muted or hidden: nothing to click, the whole strip passes clicks through.
  const away = carried && !carriedEntry() ? carried.key : null;
  const boxes = [...sprites]
    .filter(([, e]) => !e.sprite.gone && !quiet())
    // Not a sheep carried to (or drawn on) another monitor's strip.
    .filter(([id, e]) => !e.carriedAway && id !== away)
    .map(([id, { sprite }]) => {
      const { width: w, height: h } = sprite;
      // The rest position, extended upwards over the bounce, so the box doesn't move
      // every frame while a sheep bounces. Bubbles are not clickable (they pass
      // clicks through, like the rest of the strip).
      // Padded by 8 px each side so a walking sheep stays covered between reports.
      const d = sprite.drawInfo();
      return { id, x: d.x - 8, y: d.y - 10, w: w + 16, h: h + 10 };
    });
  const key = JSON.stringify(boxes);
  // At most 4 reports a second: each is an IPC round trip, and at 10 a second they
  // were a large share of the shell's CPU (Milestone 5).
  if (key === lastBoxes || (!force && now - lastBoxesAt < 250)) return;
  lastBoxes = key;
  lastBoxesAt = now;
  invoke('report_hitboxes', { monitor, boxes }).catch((e) => warn(`report_hitboxes failed: ${e}`));
}

function spriteAt(x, y) {
  // Lambs are drawn in front, so they are hit first.
  const all = [...sprites].reverse();
  for (const [id, { sprite }] of [...all.filter(([, e]) => e.parentId), ...all.filter(([, e]) => !e.parentId)]) {
    const d = sprite.drawInfo();
    if (x >= d.x && x < d.x + sprite.width && y >= d.y - 10 && y < d.y + sprite.height) return id;
  }
  return null;
}

// ------------------------------------------------------ dragging (D10)

/** A press on the canvas (or --demo-drag's): a press, a click or the start of a drag. */
function pointerDown(x, y, button = 0, pointerId) {
  // Another button going down mid-gesture: ignored (it neither clicks nor starts anything).
  if (quiet() || gesture.phase !== 'idle') return;
  const id = spriteAt(x, y);
  const entry = id && sprites.get(id);
  if (!entry || entry.carriedAway) return;
  if (entry.sprite.gone || entry.removedAt !== undefined || entry.actor.state === 'leaving') {
    // Going anyway: a press is a click at once, as before.
    click(entry);
    return;
  }
  // The moves and the release keep coming while the cursor is outside the window.
  if (button === 0 && pointerId !== undefined) {
    try {
      canvas.setPointerCapture(pointerId);
    } catch {
      // not a live pointer (a synthetic event)
    }
  }
  gestureEvent({ type: 'down', id, lamb: !!entry.parentId, button, x, y, left: entry.sprite.x });
}

function gestureEvent(ev, notify = true) {
  const r = dragEvent(gesture, ev);
  gesture = r.state;
  for (const a of r.actions) act(a, notify);
}

/** Ends any gesture under way: a carried sheep is set down where it was picked up. */
function cancelGesture(reason, notify = true) {
  if (gesture.phase !== 'idle') gestureEvent({ type: 'cancel', reason }, notify);
}

function click(entry) {
  entry.sprite.playOnce('hop', performance.now());
  invoke('sprite_clicked', { id: entry.actor.id }).catch(() => {});
}

const drag = (args) => invoke('sprite_drag', args).catch((e) => {
  warn(`sprite_drag failed: ${e}`);
  return null;
});

/** Carries out one action of the gesture's state machine (player/drag.mjs). */
function act(a, notify) {
  const now = performance.now();
  const entry = sprites.get(a.id);
  switch (a.do) {
    case 'press':
      pressId = entry.actor.id;
      drag({ phase: 'press', id: pressId });
      return;
    case 'lift': {
      const s = entry.sprite;
      s.hold(now);
      carried = { key: a.id, id: entry.actor.id, grabDx: a.grabDx, fromX: s.x, dir: s.facing };
      drag({ phase: 'lift', id: carried.id, grabDx: a.grabDx, width: s.width });
      updateHover();
      drawNow(now);
      return;
    }
    case 'carry': {
      if (!entry || !carried) return;
      // Drawn on another monitor's strip: here it stays where it was, and so do its lambs.
      if (!carriedEntry()) return;
      const before = entry.sprite.x;
      entry.sprite.carry(a.x - carried.grabDx, a.y);
      if (Math.abs(entry.sprite.x - before) >= 1) carried.dir = entry.sprite.x > before ? 1 : -1;
      drawNow(now);
      return;
    }
    case 'click':
      if (entry) click(entry);
      return;
    case 'release':
      drag({ phase: 'release', id: pressId ?? a.id });
      pressId = null;
      return;
    case 'drop':
      dropCarried(entry, now);
      return;
    case 'cancel': {
      if (notify) drag({ phase: 'cancel', id: pressId ?? a.id, reason: a.reason });
      pressId = null;
      const c = carried;
      carried = null;
      if (a.lifted && entry && entry.sprite.held && c) {
        // Set down where it was picked up; nothing is recorded.
        entry.sprite.carry(c.fromX, entry.sprite.heldY);
        entry.sprite.setDown(now);
      }
      if (a.lifted) log(`sheep ${c?.id ?? a.id} set back down: ${a.reason}`);
      dirty = true;
      return;
    }
    case 'log':
      log(a.line);
      return;
  }
}

/**
 * The drop: on this strip it lands at once where it is; carried to another strip, it is
 * hidden here and goes from here when the shell has moved it. The shell records the drop
 * (or doesn't, and the sheep is set down where it was picked up).
 */
function dropCarried(entry, now) {
  const c = carried;
  carried = null;
  const id = pressId ?? c?.id;
  pressId = null;
  if (!entry || !c) {
    drag({ phase: 'cancel', id, reason: 'its sprite went' });
    return;
  }
  const s = entry.sprite;
  const away = view.carry?.away && view.carry.id === entry.actor.id;
  const centre = s.x + s.width / 2;
  entry.dropDir = c.dir;
  if (away) {
    entry.carriedAway = true;
    entry.awayAt = now;
  } else {
    s.setDown(now);
    entry.awaitingDrop = true;
    // Until the shell's record arrives: its slot's band stays (it is usually a ghost).
    entry.localDrop = { centreX: centre, seq: Number.MAX_SAFE_INTEGER, ghost: true };
    placeFamilies();
  }
  dirty = true;
  drag({ phase: 'release', id, x: centre }).then((r) => {
    const t = performance.now();
    if (!r || !Number.isFinite(r.seq)) {
      // Not recorded (its session ended meanwhile): back where it was picked up.
      entry.localDrop = null;
      entry.awaitingDrop = false;
      if (entry.carriedAway) {
        entry.carriedAway = false;
        s.x = c.fromX;
      }
      if (s.held) s.setDown(t);
    } else if (r.here) {
      entry.localDrop = { centreX: r.centreX, seq: r.seq, ghost: !!r.ghost };
      if (entry.awaitingDrop) s.landedSeq = r.seq;
      entry.awaitingDrop = false;
      if (entry.carriedAway) {
        entry.carriedAway = false;
        s.carry(r.centreX - s.width / 2, s.heldY);
        s.setDown(t, { seq: r.seq });
      }
    } else {
      // Dropped on another monitor's strip: it goes from here with the next snapshot.
      entry.localDrop = null;
      entry.awaitingDrop = false;
      entry.carriedAway = true;
      entry.awayAt = t;
    }
    placeFamilies();
    dirty = true;
  });
}

/**
 * The shell's `carry`: a sheep carried from another monitor's strip and drawn here, at
 * the cursor less its grab offset, dangling from the cursor's y. It lives only while
 * carried; dropped here, it comes back as a sheep of this strip's flock.
 */
function applyCarry(c, now) {
  const here = c && !c.away && Number.isFinite(c.x) && Number.isFinite(c.y) && pet ? c : null;
  if (!here) {
    if (guest) {
      lastGuest = { id: guest.key, top: guest.sprite.heldY, dir: guest.dir, at: now };
      guest = null;
      dirty = true;
    }
    return;
  }
  if (!guest || guest.key !== here.id) {
    const { w, h } = spriteSize();
    const { width, height } = stripSize();
    const sprite = new Sprite({ model: pet.model, map: pet.map, width: w, height: h, stripWidth: width, stripHeight: height, home: 0, wander: 0, warn });
    sprite.hold(now);
    const colourIndex = Number.isInteger(here.colourIndex) ? here.colourIndex : 0;
    guest = { key: here.id, sprite, colourIndex, actor: { id: here.id, colourIndex, alert: here.alert ? { kind: here.alert } : null }, lastX: null, dir: 1 };
  }
  const x = here.x - (Number.isFinite(here.grabDx) ? here.grabDx : 0);
  if (guest.lastX !== null && Math.abs(x - guest.lastX) >= 1) guest.dir = x > guest.lastX ? 1 : -1;
  guest.lastX = x;
  guest.sprite.carry(x, here.y);
  drawNow(now);
}

// While a sheep is carried, the strip drawing it redraws on every move, up to the display
// rate, instead of the 150 ms cap. The cap returns at the drop.
const CARRY_DRAW_MS = 16;

function drawNow(now) {
  dirty = true;
  if (quiet() || now - lastDraw < CARRY_DRAW_MS) return;
  lastDraw = now;
  dirty = false;
  draw(now);
  lastSig = signature(now);
}

const carrying = () => !!carried || !!guest;

canvas.addEventListener('pointerdown', (e) => pointerDown(e.offsetX, e.offsetY, e.button ?? 0, e.pointerId));
// Only the button that started the gesture ends it (drag.mjs reads `buttons`, then `button`).
canvas.addEventListener('pointerup', (e) => gestureEvent({ type: 'up', x: e.offsetX, y: e.offsetY, button: e.button ?? 0, buttons: e.buttons }));
/** The live sprite (not leaving, not gone) at a point, for the hover label. */
function liveSpriteAt(x, y) {
  const id = spriteAt(x, y);
  const e = id && sprites.get(id);
  return e && !e.sprite.gone && e.removedAt === undefined ? id : null;
}

/**
 * The hover label's sprite, afresh: called every frame and whenever the pointer or the
 * shell's view changes. Never kept from an old pointer event: once the cursor is off
 * every sprite the window stops hearing the mouse, so no pointerleave would come.
 */
function updateHover() {
  const hit = quiet() ? null : hoverTarget({ pointer, over: view.over, hitAt: liveSpriteAt });
  // No hover label on a carried sheep.
  const id = hit !== null && carried && hit === carried.key ? null : hit;
  if (id !== hovered) {
    hovered = id;
    dirty = true;
  }
}

canvas.addEventListener('pointermove', (e) => {
  pointer = { x: e.offsetX, y: e.offsetY };
  if (gesture.phase !== 'idle') gestureEvent({ type: 'move', x: e.offsetX, y: e.offsetY, buttons: e.buttons });
  updateHover();
});
const forgetPointer = () => {
  pointer = null;
  updateHover();
};
canvas.addEventListener('pointerleave', forgetPointer);
// The system took the pointer (a gesture, the window losing it): forget it, and end any drag.
canvas.addEventListener('pointercancel', () => {
  forgetPointer();
  cancelGesture('pointercancel');
});
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState !== 'visible') forgetPointer();
});

// ------------------------------------------------------------------ loop

let lastSig = '';

/** Everything a redraw would show, as a string: equal strings draw the same pixels. */
function signature(now) {
  const parts = [hovered ?? '', stripSize().width, stripSize().height, muted, view.rev];
  for (const [id, e] of sprites) {
    const d = e.sprite.drawInfo(now);
    parts.push(id, e.sprite.gone ? 'g' : '', d.frame, d.x, d.y, d.flip ? 1 : 0, d.opacity, (e.grass?.alpha ?? 0).toFixed(2), e.alertKey ?? '', e.actor.hiddenLambCount ?? '', e.actor.lambAlert ? 1 : 0, e.actor.alert?.text ?? '', e.actor.label ?? '', e.carriedAway ? 'away' : '');
  }
  if (guest) {
    const d = guest.sprite.drawInfo(now);
    parts.push('guest', guest.key, d.frame, d.x, d.y, d.flip ? 1 : 0);
  }
  if (carried) parts.push('carried', carried.key, carriedEntry() ? '' : 'away');
  return parts.join('|');
}

const DRAW_INTERVAL_MS = 150;
// A crowded strip (lambs: Milestone 6) is always dirty, so it redraws at the cap; a
// 200 ms step there keeps 3 sheep with 3 lambs each inside the 5% CPU budget.
const CROWDED_DRAW_INTERVAL_MS = 200;
const CROWDED_SPRITES = 6;
let lastBounceDraw = 0;
let lastDraw = 0;
let bounceDue = false;

function frame() {
  const now = performance.now();
  if (quiet()) {
    pointer = null;
    updateHover();
    // No ticking, no bounce: draw once when something changed (a badge to show).
    if (dirty) {
      dirty = false;
      const sig = `${muted}|${view.hidden}|` + signature(now);
      if (sig !== lastSig) {
        lastSig = sig;
        draw(now);
      }
    }
    reportHitboxes(now);
    return;
  }
  const step = carrying() ? CARRY_DRAW_MS : sprites.size > CROWDED_SPRITES ? CROWDED_DRAW_INTERVAL_MS : DRAW_INTERVAL_MS;
  const { width } = stripSize();
  if (guest && guest.sprite.tick(now)) dirty = true;
  for (const entry of sprites.values()) {
    // A lamb's place moves with its parent.
    const parent = entry.parentId && sprites.get(entry.parentId);
    if (parent && entry.removedAt === undefined) {
      // Wherever the parent goes, the lamb's place goes too; the sprite walks (or,
      // asleep or alerting, slides) back to it (sprite.mjs).
      entry.sprite.setHome(lambX(entry.place, { parentX: parent.sprite.x, parentW: parent.sprite.width, lambW: entry.sprite.width, stripWidth: width }));
    }
  }
  let replace = false;
  for (const [id, entry] of sprites) {
    if (entry.sprite.tick(now)) dirty = true;
    if (stepGrass(entry, now)) dirty = true;
    // The bounce needs a redraw even when no frame changed (see DRAW_INTERVAL_MS).
    if (entry.sprite.bouncing && now - lastBounceDraw >= step) {
      dirty = true;
      bounceDue = true;
    }
    if (entry.removedAt !== undefined && (entry.sprite.gone || now - entry.removedAt > 4000)) {
      sprites.delete(id);
      dirty = true;
      replace = true;
    }
    if (entry.carriedAway && now - entry.awayAt > 3000) {
      // Carried away but still in this strip's flock after 3 s: show it here again.
      entry.carriedAway = false;
      if (entry.sprite.held) entry.sprite.setDown(now);
      dirty = true;
      replace = true;
    }
  }
  if (replace) placeFamilies();
  // After the sprites have moved: a sheep may have walked out from under a still cursor.
  updateHover();
  if (bounceDue) lastBounceDraw = now;
  bounceDue = false;
  // At most one redraw every 150 ms. Each redraw costs about the same whatever is
  // drawn and however many pixels (WebKit's per-frame commit, measured in Milestone 5:
  // ~0.5% CPU per redraw per second), so the rate is what keeps the strip within
  // budget. esheep's frames last 100–200 ms, so little is lost.
  if (dirty && now - lastDraw >= step) {
    lastDraw = now;
    dirty = false;
    // A tick often changes nothing visible (an animation repeating its frame, a sheep
    // turning at the edge of its range): skip the redraw, WebKit's commit is the cost.
    const sig = signature(now);
    if (sig !== lastSig) {
      lastSig = sig;
      draw(now);
    }
  }
  // Checked every frame (it does nothing when the boxes haven't changed), so a sheep's
  // final position is reported even when no redraw follows its last step.
  reportHitboxes(now);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function boot() {
  resize();
  log(`webview geometry: inner ${window.innerWidth}x${window.innerHeight} at screen ${window.screenX},${window.screenY}, devicePixelRatio ${window.devicePixelRatio}, screen ${screen.width}x${screen.height}`);
  window.addEventListener('resize', resize);
  const missing = [];
  const realWarn = warn;
  try {
    await loadPet((m) => {
      missing.push(m);
      realWarn(m);
    });
  } catch (e) {
    log(`error: ${e.message ?? e}`);
    // Only acted on when the shell runs --selftest; otherwise just a log line.
    log(`selftest: done FAIL (${e.message ?? e})`);
    return;
  }
  if (display.selftest) {
    // --selftest: the real DOMParser on the real pet, then exit (the shell quits on "done").
    log(`selftest: ${pet.model.title}: ${Object.keys(pet.model.animations).length} animations, ${pet.tileW}x${pet.tileH} tiles, ${pet.sheets.length} tinted sheets`);
    log(`selftest: behaviours mapped: ${Object.entries(pet.map).map(([b, ids]) => `${b}=${ids.map((i) => pet.model.animations[i].name).join('+')}`).join(' ')}`);
    for (const m of missing) log(`selftest: missing: ${m}`);
    // The strip's capability doesn't grant the onboarding window's commands (plan D6): a
    // call from here must be refused before it reaches Rust.
    const refused = await invoke('setup_status').then(() => null, (e) => String(e?.message ?? e));
    log(refused ? `selftest: onboarding command refused to the strip: ${refused}` : 'selftest: FAIL the strip reached setup_status');
    if (!refused) missing.push('the strip reached an onboarding command');
    log(`selftest: done ${missing.length ? 'FAIL' : 'PASS'}`);
    return;
  }
  // 20 Hz is enough: esheep frames last 100–200 ms. A timer, not
  // requestAnimationFrame, keeps the CPU near zero when nothing moves.
  setInterval(frame, 50);
  if (display.trace) {
    // BELLWETHER_SHELL_TRACE: a heartbeat, to tell a busy webview from a throttled one.
    let beats = 0;
    setInterval(() => log(`trace: heartbeat ${++beats}, ${sprites.size} sprites, page ${document.visibilityState}`), 1000);
    document.addEventListener('visibilitychange', () => log(`trace: visibility -> ${document.visibilityState}`));
  }
  follow();
  if (display.demoDrag) demoDrag();
  if (display.snapshotAfterMs) setTimeout(snapshots, display.snapshotAfterMs);
  if (display.snapshotEveryMs) {
    let n = 0;
    setInterval(() => snapshots(`t${String(++n).padStart(2, '0')}`), display.snapshotEveryMs);
  }
}

/**
 * Debug (`--demo --demo-drag`): a scripted drag through the same state machine and the same
 * shell command as a real one. It presses demo sheep 2, lifts it, carries it 300 px over
 * 2 s towards sheep 4, drops it onto sheep 4, then clicks sheep 1 without moving. With
 * `--snapshot-every 1` it is the offline view of the feature.
 */
async function demoDrag() {
  const centreOf = (id) => {
    const e = sprites.get(id);
    const d = e.sprite.drawInfo();
    return { x: d.x + e.sprite.width / 2, y: d.y + e.sprite.height / 2 };
  };
  for (let i = 0; i < 100 && !['s-demo1', 's-demo2', 's-demo4'].every((id) => sprites.has(id)); i++) await sleep(100);
  if (!['s-demo1', 's-demo2', 's-demo4'].every((id) => sprites.has(id))) {
    warn('demo-drag: demo sheep 1, 2 and 4 are not on this strip');
    return;
  }
  await sleep(2500);
  const from = centreOf('s-demo2');
  log(`demo-drag: press s-demo2 at x ${Math.round(from.x)}`);
  pointerDown(from.x, from.y);
  await sleep(300);
  const dir = Math.sign(centreOf('s-demo4').x - from.x) || 1;
  for (let k = 1; k <= 40; k++) {
    await sleep(50);
    gestureEvent({ type: 'move', x: from.x + (dir * 300 * k) / 40, y: from.y - 30 * Math.sin((Math.PI * k) / 40) });
  }
  const onto = centreOf('s-demo4');
  gestureEvent({ type: 'move', x: onto.x, y: from.y - 10 });
  await sleep(150);
  log(`demo-drag: drop onto s-demo4 at x ${Math.round(onto.x)} (carried ${Math.round(Math.abs(onto.x - from.x))} px)`);
  gestureEvent({ type: 'up', x: onto.x, y: from.y - 10 });
  await sleep(4000);
  const one = centreOf('s-demo1');
  log('demo-drag: click s-demo1 without moving');
  pointerDown(one.x, one.y);
  await sleep(150);
  gestureEvent({ type: 'up', x: one.x, y: one.y });
}

// Debug (--snapshot): the canvas as it is, and the same over a checkerboard so the
// transparency is visible in an image viewer.
async function snapshots(tag = '') {
  const plain = canvas.toDataURL('image/png');
  const c = document.createElement('canvas');
  c.width = canvas.width;
  c.height = canvas.height;
  const g = c.getContext('2d');
  const cell = 16 * (window.devicePixelRatio || 1);
  for (let y = 0; y < c.height; y += cell) for (let x = 0; x < c.width; x += cell) {
    g.fillStyle = (x / cell + y / cell) % 2 ? '#c8c8c8' : '#e8e8e8';
    g.fillRect(x, y, cell, cell);
  }
  g.drawImage(canvas, 0, 0);
  await invoke('debug_snapshot', { png: plain, suffix: tag }).catch((e) => warn(`snapshot: ${e}`));
  await invoke('debug_snapshot', { png: c.toDataURL('image/png'), suffix: tag ? `${tag}-checker` : 'checker' }).catch((e) => warn(`snapshot: ${e}`));
}

boot();
