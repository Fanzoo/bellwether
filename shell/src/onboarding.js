// The onboarding window (distribution plan D6, milestone 7). It calls only its own eleven
// commands (capabilities/onboarding.json): setup_status, hooks_preview, hooks_apply,
// login_items_set, permission_request, pet_select, pet_import, migration_preview,
// migration_apply, cli_tool_install, setup_close. Arguments are enums or opaque ids; the
// Rust side does every read and write, and masks the hook token in what it returns.
// Text from files (pet titles, paths) only ever goes in through textContent.

const invoke = window.__TAURI__.core.invoke;
const $ = (id) => document.getElementById(id);

let status = null;
let steps = [];
let at = 0;
let plan = null; // the current hook preview
let move = null; // the current migration preview (step 1)
let moved = false; // step 1 ran in this window: steps 2–3 stay replaced

// ---------------------------------------------------------------- helpers

function say(id, text, error = false) {
  const el = $(id);
  el.textContent = text ?? '';
  el.classList.toggle('error', Boolean(error));
}

function lines(id, items) {
  const ul = $(id);
  ul.replaceChildren(...items.map((t) => {
    const li = document.createElement('li');
    li.textContent = t;
    return li;
  }));
}

/** Runs a command with the buttons of its step disabled; errors go to `resultId`. */
async function act(resultId, fn) {
  const section = document.querySelector(`section[data-step="${steps[at]}"]`);
  const buttons = [...section.querySelectorAll('button')];
  buttons.forEach((b) => { b.disabled = true; });
  const enabled = buttons.filter((b) => b.id !== 'hooks-apply' && b.id !== 'earlier-apply');
  say(resultId, 'Working…');
  try {
    await fn();
  } catch (e) {
    say(resultId, String(e?.message ?? e), true);
  } finally {
    // render() decides Install (enabled only with a current plan).
    enabled.forEach((b) => { b.disabled = false; });
    render();
  }
}

async function refresh() {
  status = await invoke('setup_status');
}

// ---------------------------------------------------------------- steps

function stepList() {
  // Step 1 (moving an earlier install) replaces steps 2–3 (plan D6).
  if (status.migration?.found || moved) return ['earlier', 'dock', 'focus', 'pet', 'cli', 'done'];
  return ['login', 'hooks', 'dock', 'focus', 'pet', 'cli', 'done'];
}

const LOGIN_WORDS = { enabled: 'on', notRegistered: 'off', notFound: 'off', requiresApproval: 'waiting for your approval', unavailable: 'not available here', unknown: 'unknown' };
const AUTOMATION_WORDS = {
  allowed: 'allowed',
  denied: 'refused (System Settings > Privacy & Security > Automation can change it)',
  notAsked: 'not asked yet',
  notRunning: 'not running (start it, then ask)',
  unknown: 'unknown',
};
const DOCTOR_WORDS = { PASS: 'OK', INFO: 'Note', WARN: 'Worth a look', FAIL: 'Needs fixing' };

function render() {
  if (!status) return;
  steps = stepList();
  at = Math.min(at, steps.length - 1);
  for (const s of document.querySelectorAll('section[data-step]')) s.hidden = s.dataset.step !== steps[at];
  $('progress').textContent = `Step ${at + 1} of ${steps.length}`;
  $('back').disabled = at === 0;
  $('next').hidden = at === steps.length - 1;
  $('finish').hidden = at !== steps.length - 1;

  const earlier = status.migration?.found;
  const agents = status.migration?.agents ?? [];
  $('earlier-text').textContent = moved && !earlier ? 'Moved into the app.' : earlier ? (agents.length ? `Found: ${agents.join(', ')}.` : 'Found: Bellwether hook entries from an earlier install.') : '';
  $('earlier-plan').hidden = !move?.text;
  $('earlier-plan').textContent = move?.text ?? '';
  $('earlier-apply').disabled = !move?.planId;

  // Start at login.
  const login = status.login;
  if (!login) {
    lines('login-lines', ['This is a repository build: start at login is set up with ./bellwether autostart.']);
  } else {
    lines('login-lines', [`Brain at login: ${LOGIN_WORDS[login.brain] ?? login.brain}`, `Strip at login: ${LOGIN_WORDS[login.app] ?? login.app}`, ...(login.waiting || login.stale ? login.lines : [])]);
  }
  $('login-on').disabled = !login || earlier;
  $('login-off').disabled = !login;
  $('login-approve').hidden = !login?.waiting;

  // Hooks.
  const h = status.hooks;
  $('hooks-warn-login').hidden = !(login && login.brain !== 'enabled');
  $('hooks-state').textContent = h.problem
    ? `${h.file}: ${h.problem}`
    : h.upToDate
      ? `Installed: Bellwether's hooks are in ${h.file} (${h.groups} of ${h.events} events).`
      : h.groups > 0
        ? `Partly installed or out of date in ${h.file} (${h.groups} of ${h.events} events): show the changes to bring it up to date.`
        : `Not installed in ${h.file} yet.`;
  $('hooks-preview-install').disabled = Boolean(earlier);
  $('hooks-plan').hidden = !plan?.planId;
  $('hooks-apply').disabled = !plan?.planId;
  if (plan) {
    $('hooks-summary').textContent = `${plan.summary ?? ''} Other tools' hooks left untouched: ${plan.othersUntouched}.${plan.newToken ? ' A new hook token is made with the install (shown masked).' : ''}`;
    lines('hooks-warnings', plan.warnings ?? []);
    $('hooks-diff').textContent = plan.diffText;
    $('hooks-apply').textContent = plan.action === 'uninstall' ? 'Uninstall' : 'Install';
  }

  // Dock.
  $('dock-state').textContent = status.accessibility ? 'Allowed: sheep stand on the Dock.' : 'Not allowed: sheep use the full width above the Dock.';
  $('dock-ask').disabled = status.accessibility;

  // Click to focus.
  lines('focus-state', [`iTerm2: ${AUTOMATION_WORDS[status.automation.iterm2]}`, `Terminal: ${AUTOMATION_WORDS[status.automation.terminal]}`]);

  // Pet.
  renderPets();

  // CLI tool.
  const c = status.cliTool;
  $('cli-state').textContent = {
    installed: `Installed: ${c.path}.`,
    homebrew: c.detail,
    otherApp: `${c.path} points at another copy of Bellwether.app; installing points it at this one.`,
    foreign: `${c.path} is already there and isn't Bellwether's; it is left alone.`,
    absent: 'Not installed.',
    unavailable: c.detail,
  }[c.state] ?? c.state;
  $('cli-install').disabled = ['installed', 'homebrew', 'foreign', 'unavailable'].includes(c.state);

  // Done: doctor in plain words.
  const checks = status.doctor ?? [];
  const fails = checks.filter((x) => x.status === 'FAIL');
  $('done-summary').textContent = fails.length
    ? `Almost there: ${fails.length === 1 ? 'one thing needs' : `${fails.length} things need`} fixing before sheep appear (below).`
    : 'Everything needed is in place. Start a new Claude Code session: its sheep appears above the Dock.';
  const shown = checks.filter((x) => x.name !== 'next' || fails.length);
  $('doctor').replaceChildren(...shown.map((x) => {
    const li = document.createElement('li');
    const word = document.createElement('span');
    word.className = 'word';
    word.textContent = DOCTOR_WORDS[x.status] ?? x.status;
    li.append(word, `${x.name}: ${x.detail}`);
    return li;
  }));
}

function renderPets() {
  const pets = status.pets;
  $('pet-intro').textContent = pets.list.length
    ? 'Choose the pet the strip draws. Every session gets its own colour.'
    : 'No pet is installed yet. Use Import… with a desktopPet pet file (animations.xml) you have.';
  $('pet-import').disabled = !pets.editable;
  $('pet-list').replaceChildren(...pets.list.map((p) => {
    const li = document.createElement('li');
    li.classList.toggle('current', p.name === pets.current);
    const title = document.createElement('div');
    title.textContent = p.title ? `${p.title} (${p.name})` : p.name;
    if (p.still) li.append(still(p.still, pets.tint));
    li.append(title);
    if (p.problem) {
      const why = document.createElement('div');
      why.className = 'result error';
      why.textContent = p.problem;
      li.append(why);
    } else if (p.name === pets.current) {
      const now = document.createElement('div');
      now.className = 'quiet';
      now.textContent = 'in use';
      li.append(now);
    } else {
      const b = document.createElement('button');
      b.textContent = 'Use this pet';
      b.disabled = !pets.editable;
      b.addEventListener('click', () => act('pet-result', async () => {
        const r = await invoke('pet_select', { name: p.name });
        status.pets = r.pets;
        say('pet-result', `The strip now draws ${p.name}.`);
      }));
      li.append(b);
    }
    return li;
  }));
}

/**
 * The strip's tinter (player/horns.mjs `tintHorns`), copied: this page is a classic script
 * and loads nothing else, so it can't import it. test/pets-bundled.test.mjs checks the copy
 * recolours exactly as the original does. The mid magenta becomes `hex`, the dark and
 * light tones darker and lighter; nothing outside the mask's hue window changes.
 */
function tintMask(data, hex) {
  const m = /^#?([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex ?? '');
  if (!m) return 0;
  const t = m.slice(1).map((x) => Number.parseInt(x, 16));
  let n = 0;
  for (let i = 0; i < data.length; i += 4) {
    const [r, g, b, a] = [data[i], data[i + 1], data[i + 2], data[i + 3]];
    if (a < 128 || (r > 240 && g < 16 && b > 240)) continue;
    const max = Math.max(r, g, b);
    const c = max - Math.min(r, g, b);
    let h = 0;
    if (c) {
      if (max === r) h = ((g - b) / c) % 6;
      else if (max === g) h = (b - r) / c + 2;
      else h = (r - g) / c + 4;
      h *= 60;
      if (h < 0) h += 360;
    }
    const s = max ? c / max : 0;
    if (!(s >= 0.35 && h >= 285 && h <= 315)) continue;
    const white = 1 - s;
    const shade = Math.min(1, max / 255 / (0xb3 / 255));
    for (let k = 0; k < 3; k++) {
      const shaded = t[k] * shade;
      data[i + k] = Math.round(shaded + (255 - shaded) * white);
    }
    n++;
  }
  return n;
}

/**
 * A canvas showing one tile of a pet sheet, its transparency colour keyed out, and its
 * helmet (the mask) tinted with `tint`, the first palette colour, so no one sees the
 * mask's magenta (plan D8).
 */
function still(s, tint) {
  const c = document.createElement('canvas');
  const img = new Image();
  img.addEventListener('load', () => {
    const w = Math.floor(img.width / s.tilesx);
    const h = Math.floor(img.height / s.tilesy);
    c.width = w;
    c.height = h;
    const g = c.getContext('2d');
    g.drawImage(img, (s.frame % s.tilesx) * w, Math.floor(s.frame / s.tilesx) * h, w, h, 0, 0, w, h);
    const d = g.getImageData(0, 0, w, h);
    if (/^magenta$/i.test(s.transparency)) {
      for (let i = 0; i < d.data.length; i += 4) {
        if (d.data[i] === 255 && d.data[i + 1] === 0 && d.data[i + 2] === 255) d.data[i + 3] = 0;
      }
    }
    if (tint) tintMask(d.data, tint);
    g.putImageData(d, 0, 0);
  });
  img.src = `data:image/png;base64,${s.png}`;
  return c;
}

// ---------------------------------------------------------------- actions

$('back').addEventListener('click', () => { at = Math.max(0, at - 1); render(); });
$('next').addEventListener('click', async () => {
  at = Math.min(steps.length - 1, at + 1);
  if (steps[at] === 'done') {
    say('done-summary', 'Checking…');
    await refresh().catch(() => {});
  }
  render();
});
$('finish').addEventListener('click', () => invoke('setup_close').catch((e) => say('done-summary', String(e), true)));

async function loginSet(enabled) {
  await act('login-result', async () => {
    const r = await invoke('login_items_set', { enabled });
    say('login-result', r.lines.join('\n'));
    await refresh();
  });
}
$('login-on').addEventListener('click', () => loginSet(true));
$('login-off').addEventListener('click', () => loginSet(false));
$('login-approve').addEventListener('click', () => act('login-result', async () => {
  say('login-result', (await invoke('permission_request', { kind: 'login-items' })).message);
}));

async function showMove() {
  await act('earlier-result', async () => {
    move = await invoke('migration_preview');
    say('earlier-result', move.planId ? '' : move.message);
  });
}
$('earlier-preview').addEventListener('click', () => showMove());
$('earlier-apply').addEventListener('click', () => act('earlier-result', async () => {
  // A preview is used once, as with the hooks.
  const id = move?.planId;
  move = null;
  render();
  if (!id) return;
  const r = await invoke('migration_apply', { planId: id });
  if (r.outcome === 'changed') {
    move = r.preview?.planId ? r.preview : null;
    say('earlier-result', r.message, true);
    return;
  }
  if (r.outcome === 'applied') moved = true;
  say('earlier-result', r.text, r.outcome !== 'applied');
  await refresh();
}));

async function preview(action) {
  await act('hooks-result', async () => {
    plan = await invoke('hooks_preview', { action });
    say('hooks-result', plan.planId ? '' : plan.message);
  });
}
$('hooks-preview-install').addEventListener('click', () => preview('install'));
$('hooks-preview-uninstall').addEventListener('click', () => preview('uninstall'));
$('hooks-apply').addEventListener('click', () => act('hooks-result', async () => {
  // A plan is used once: whatever the answer, this one is gone. Until a fresh preview
  // arrives (with a refusal, or from Show the changes) there is nothing to Install.
  const id = plan?.planId;
  plan = null;
  render();
  if (!id) return;
  const r = await invoke('hooks_apply', { planId: id });
  if (r.outcome === 'changed') {
    plan = r.preview?.planId ? r.preview : null;
    say('hooks-result', r.message, true);
    return;
  }
  plan = null;
  say('hooks-result', [r.summary, r.backup ? `Backup of the previous file: ${r.backup}` : '', ...(r.warnings ?? [])].filter(Boolean).join('\n'));
  await refresh();
}));

$('dock-ask').addEventListener('click', () => act('dock-result', async () => {
  say('dock-result', (await invoke('permission_request', { kind: 'accessibility' })).message);
  await refresh();
}));

function automation(kind) {
  return () => act('focus-result', async () => {
    const r = await invoke('permission_request', { kind });
    say('focus-result', r.message, !r.ok);
    await refresh();
  });
}
$('focus-iterm2').addEventListener('click', automation('automation-iterm2'));
$('focus-terminal').addEventListener('click', automation('automation-terminal'));

$('pet-import').addEventListener('click', () => act('pet-result', async () => {
  const r = await invoke('pet_import');
  if (r.cancelled) {
    say('pet-result', '');
    return;
  }
  say('pet-result', `Imported ${r.name} (${r.animations} animations). Choose it below to use it.`);
  await refresh();
}));

$('cli-install').addEventListener('click', () => act('cli-result', async () => {
  const r = await invoke('cli_tool_install');
  say('cli-result', r.detail ?? r.state);
  await refresh();
}));

// ---------------------------------------------------------------- start

(async () => {
  try {
    await refresh();
    render();
    if (status.migration?.found) await showMove();
  } catch (e) {
    document.body.textContent = `Bellwether couldn't read its setup: ${e?.message ?? e}`;
  }
})();
