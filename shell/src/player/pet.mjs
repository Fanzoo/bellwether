/**
 * PetModel: the parts of a desktopPet `animations.xml` that bellwether uses, as plain
 * data. `parsePetDocument` reads a parsed XML document (the webview's DOMParser); the
 * rest of the player works on the model alone, so `node --test` covers it with
 * test/fixtures/pet-model.json and no DOM.
 *
 * Format reference: desktopPet's Resources/animations.xsd (MIT). Not derived from
 * web-esheep's esheep.js (GPL-3.0); see NOTICE.
 *
 * {
 *   title, tilesx, tilesy, png (base64), transparency ("Magenta" or none),
 *   animations: { [id]: { id, name, start: {x, y, interval, offsety, opacity},
 *                         end: {…}, frames: [n…], repeat: "expr", repeatfrom: n,
 *                         next: [{id, probability, only}], action? } },
 *   byName: { [name]: id }
 * }
 */

const MAX_ANIMATIONS = 500;
const MAX_FRAMES = 2000;

/**
 * Throws if the XML text declares a DOCTYPE or ENTITY. Pet files never need one, and
 * refusing them before DOMParser runs rules out entity expansion. (The shell checks
 * the same in Rust, and the file's pinned sha256, before handing the text over.)
 */
export function assertSafePetXml(text) {
  if (/<!doctype|<!entity/i.test(text)) throw new Error('the pet file declares a DOCTYPE or ENTITY, which pet files never need; refusing it');
  return text;
}

/** Reads the model from a parsed XML Document (or anything with the same small API). */
export function parsePetDocument(doc) {
  const root = doc.documentElement;
  if (!root || local(root) !== 'animations') throw new Error('not a pet file: the root element must be <animations>');
  const child = (el, name) => [...el.children].find((c) => local(c) === name) ?? null;
  const children = (el, name) => [...el.children].filter((c) => local(c) === name);
  const text = (el, name, fallback = '') => {
    const c = el && child(el, name);
    return c ? c.textContent.trim() : fallback;
  };

  const header = child(root, 'header');
  const image = child(root, 'image');
  if (!image) throw new Error('pet file has no <image>');
  const model = {
    title: header ? text(header, 'title') : '',
    tilesx: Number(text(image, 'tilesx')),
    tilesy: Number(text(image, 'tilesy')),
    png: text(image, 'png').replace(/\s+/g, ''),
    transparency: text(image, 'transparency', ''),
    animations: {},
    byName: {},
  };
  const anims = child(root, 'animations');
  for (const a of anims ? children(anims, 'animation').slice(0, MAX_ANIMATIONS) : []) {
    const id = Number(a.getAttribute('id'));
    if (!Number.isInteger(id)) continue;
    const seq = child(a, 'sequence');
    const pos = (el) => ({
      x: el ? text(el, 'x', '0') : '0',
      y: el ? text(el, 'y', '0') : '0',
      interval: el ? text(el, 'interval', '200') : '200',
      offsety: el ? Number(text(el, 'offsety', '0')) || 0 : 0,
      opacity: el ? Number(text(el, 'opacity', '1')) || 1 : 1,
    });
    const nexts = (el) =>
      el
        ? children(el, 'next').map((n) => ({
            id: Number(n.textContent.trim()),
            probability: Number(n.getAttribute('probability') ?? '100'),
            only: n.getAttribute('only') ?? 'none',
          }))
        : [];
    const anim = {
      id,
      name: text(a, 'name'),
      start: pos(child(a, 'start')),
      end: pos(child(a, 'end')),
      frames: seq ? children(seq, 'frame').slice(0, MAX_FRAMES).map((f) => Number(f.textContent.trim())) : [],
      repeat: seq?.getAttribute('repeat') ?? '0',
      repeatfrom: Number(seq?.getAttribute('repeatfrom') ?? '0') || 0,
      next: nexts(seq),
      action: seq ? text(seq, 'action', '') || undefined : undefined,
    };
    model.animations[id] = anim;
    if (anim.name && model.byName[anim.name] === undefined) model.byName[anim.name] = id;
  }
  validatePet(model);
  return model;
}

/** Throws with a reason if the model can't be played. */
export function validatePet(model) {
  if (!Number.isInteger(model.tilesx) || model.tilesx < 1 || model.tilesx > 64) throw new Error('tilesx must be an integer from 1 to 64');
  if (!Number.isInteger(model.tilesy) || model.tilesy < 1 || model.tilesy > 64) throw new Error('tilesy must be an integer from 1 to 64');
  const ids = Object.keys(model.animations);
  if (!ids.length) throw new Error('pet file has no animations');
  const maxFrame = model.tilesx * model.tilesy;
  for (const a of Object.values(model.animations)) {
    a.frames = a.frames.filter((f) => Number.isInteger(f) && f >= 0 && f < maxFrame);
    if (!Array.isArray(a.next)) a.next = [];
    a.next = a.next.filter((n) => Number.isInteger(n.id) && model.animations[n.id] && Number.isFinite(n.probability) && n.probability >= 0);
    if (!(a.repeatfrom >= 0 && a.repeatfrom < Math.max(1, a.frames.length))) a.repeatfrom = 0;
  }
  return model;
}

/**
 * Behaviour names (from config `pets.animationMap`) resolved to animation ids. A name
 * the pet doesn't have is reported once and dropped; a behaviour with nothing left
 * falls back to `fallback` (walk).
 */
export function resolveAnimationMap(model, map, warn = () => {}) {
  const out = {};
  for (const [behaviour, names] of Object.entries(map ?? {})) {
    const ids = (Array.isArray(names) ? names : [names])
      .map((n) => {
        const id = model.byName[n];
        if (id === undefined) warn(`pet "${model.title}" has no animation "${n}" (behaviour ${behaviour})`);
        return id;
      })
      .filter((id) => id !== undefined && model.animations[id].frames.length > 0);
    if (ids.length) out[behaviour] = ids;
  }
  return out;
}

function local(el) {
  return (el.localName ?? el.tagName ?? '').replace(/^.*:/, '');
}
