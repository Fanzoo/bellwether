import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { describe, test } from 'node:test';
import { AGENT_DIR } from './helpers.mjs';

/**
 * The onboarding window's page (plan D6, milestone 7): shell/src/onboarding.js run against
 * a small stand-in for the DOM built from shell/src/onboarding.html, with `invoke` answered
 * here. It checks what the page calls and what it shows; the commands themselves are
 * cargo-tested (shell/src-tauri/src/onboarding.rs, tests/onboarding_preview.rs).
 */

const SRC = path.join(AGENT_DIR, 'shell', 'src');
const HTML = fs.readFileSync(path.join(SRC, 'onboarding.html'), 'utf8');
const JS = fs.readFileSync(path.join(SRC, 'onboarding.js'), 'utf8');
const ELEVEN = ['setup_status', 'hooks_preview', 'hooks_apply', 'login_items_set', 'permission_request', 'pet_select', 'pet_import', 'migration_preview', 'migration_apply', 'cli_tool_install', 'setup_close'];

class El {
  constructor(tag, id = null) {
    this.tag = tag;
    this.id = id;
    this.textContent = '';
    this.hidden = false;
    this.disabled = false;
    this.children = [];
    this.dataset = {};
    this.listeners = {};
    this.className = '';
    const classes = new Set();
    this.classList = { toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)), contains: (c) => classes.has(c) };
  }
  replaceChildren(...c) { this.children = c; }
  append(...c) { this.children.push(...c); }
  addEventListener(type, fn) { (this.listeners[type] ??= []).push(fn); }
  async click() { for (const fn of this.listeners.click ?? []) await fn(); }
  getContext() { return { drawImage() {}, getImageData: () => ({ data: [] }), putImageData() {} }; }
  /** All text below this element, as a person would read it. */
  text() { return [this.textContent, ...this.children.map((c) => (typeof c === 'string' ? c : c.text()))].join(' '); }
}

/** A DOM for onboarding.html: every element with an id, the sections and their buttons. */
function page() {
  const byId = new Map();
  const sections = [];
  let current = null;
  for (const m of HTML.matchAll(/<(\w+)([^>]*)>/g)) {
    const [, tag, attrs] = m;
    const id = /\bid="([^"]+)"/.exec(attrs)?.[1];
    const step = /\bdata-step="([^"]+)"/.exec(attrs)?.[1];
    const el = new El(tag, id);
    el.hidden = /\bhidden\b/.test(attrs);
    if (step) {
      el.dataset.step = step;
      el.buttons = [];
      sections.push(el);
      current = el;
    }
    if (tag === 'footer') current = null;
    if (tag === 'button' && current) current.buttons.push(el);
    if (id) byId.set(id, el);
  }
  const body = new El('body');
  return {
    byId,
    sections,
    document: {
      body,
      getElementById: (id) => byId.get(id),
      createElement: (tag) => new El(tag),
      querySelectorAll: (sel) => (sel === 'section[data-step]' ? sections : []),
      querySelector: (sel) => {
        const step = /^section\[data-step="([^"]+)"\]$/.exec(sel)?.[1];
        const s = sections.find((x) => x.dataset.step === step);
        return s && { querySelectorAll: () => s.buttons };
      },
    },
  };
}

function status(over = {}) {
  return {
    layout: 'app',
    port: 47431,
    onboarded: false,
    token: { usable: false, missing: true },
    hooks: { file: '/Users/new/.claude/settings.json', groups: 0, events: 16, upToDate: false, problem: null },
    login: { stale: false, brain: 'notRegistered', app: 'notRegistered', waiting: false, lines: ['brain agent: off', 'app at login: off'] },
    accessibility: false,
    automation: { iterm2: 'notRunning', terminal: 'notAsked' },
    pets: { current: 'bellwether', list: [], editable: true },
    migration: { found: false, agents: [] },
    cliTool: { state: 'absent', path: '/Users/new/.local/bin/bellwether', dir: '/Users/new/.local/bin' },
    doctor: [{ status: 'PASS', name: 'config', detail: 'the defaults' }, { status: 'FAIL', name: 'brain', detail: 'brain not running' }],
    ...over,
  };
}

/** Loads onboarding.js into a fresh page; `answer(cmd, args)` plays the Rust side. */
async function load(answer) {
  const p = page();
  const calls = [];
  const invoke = async (cmd, args = {}) => {
    // Copied into this realm (the page runs in its own context), so deepEqual compares values.
    const plain = JSON.parse(JSON.stringify(args));
    calls.push([cmd, plain]);
    return answer(cmd, plain);
  };
  // A fresh run of the page's script in its own context, with this page as its globals.
  vm.runInNewContext(JS, { document: p.document, window: { __TAURI__: { core: { invoke } } }, Image: class { addEventListener() {} set src(_) {} }, setTimeout, console });
  await settle();
  const visible = () => p.sections.filter((s) => !s.hidden).map((s) => s.dataset.step);
  const el = (id) => p.byId.get(id);
  return { p, calls, visible, el, async click(id) { await el(id).click(); await settle(); } };
}

const settle = () => new Promise((r) => setTimeout(r, 10));

describe('the onboarding page (plan D6)', () => {
  test('it calls only its own eleven commands, and writes text only through textContent', () => {
    const used = new Set([...JS.matchAll(/invoke\('([a-z_]+)'/g)].map((m) => m[1]));
    for (const c of used) assert.ok(ELEVEN.includes(c), `${c} is not an onboarding command`);
    assert.doesNotMatch(JS, /innerHTML|outerHTML|insertAdjacentHTML|document\.write|eval\(|new Function/);
    assert.doesNotMatch(HTML, /<script(?![^>]*src="onboarding\.js")|style="|on[a-z]+="/, 'no inline script, style or handler (the CSP refuses them)');
    for (const c of ['get_pet', 'get_snapshot', 'report_hitboxes', 'sprite_clicked', 'shell_log', 'debug_snapshot']) assert.ok(!used.has(c), `the page must not call the strip's ${c}`);
  });

  test('a new user walks steps 2–8 in order, and Done ends with doctor in plain words', async () => {
    const w = await load((cmd) => (cmd === 'setup_status' ? status() : {}));
    assert.deepEqual(w.visible(), ['login']);
    assert.equal(w.el('progress').textContent, 'Step 1 of 7');
    const seen = [];
    for (let i = 0; i < 6; i++) {
      seen.push(w.visible()[0]);
      await w.click('next');
    }
    seen.push(w.visible()[0]);
    assert.deepEqual(seen, ['login', 'hooks', 'dock', 'focus', 'pet', 'cli', 'done']);
    assert.equal(w.el('finish').hidden, false);
    assert.match(w.el('done-summary').textContent, /one thing needs fixing/);
    const doctor = w.el('doctor').text();
    assert.match(doctor, /OK config: the defaults/);
    assert.match(doctor, /Needs fixing brain: brain not running/);
    await w.click('finish');
    assert.deepEqual(w.calls.at(-1), ['setup_close', {}]);
  });

  test('without start at login, the hooks step warns (Q12), and an earlier install adds step 1 and disables both', async () => {
    let w = await load(() => status());
    assert.equal(w.el('hooks-warn-login').hidden, false);
    w = await load(() => status({ login: { stale: false, brain: 'enabled', app: 'enabled', waiting: false, lines: [] } }));
    assert.equal(w.el('hooks-warn-login').hidden, true);
    w = await load((cmd) => (cmd === 'setup_status' ? status({ migration: { found: true, agents: ['local.bellwether.brain (running)'] } }) : { found: true, planId: null, text: '' }));
    assert.deepEqual(w.visible(), ['earlier']);
    // Step 1 replaces steps 2–3 (plan D6): six steps, no login or hooks page.
    assert.equal(w.el('progress').textContent, 'Step 1 of 6');
    assert.match(w.el('earlier-text').textContent, /local\.bellwether\.brain \(running\)/);
    assert.equal(w.el('login-on').disabled, true);
    assert.equal(w.el('hooks-preview-install').disabled, true);
    const seen = [];
    for (let i = 0; i < 5; i++) {
      seen.push(w.visible()[0]);
      await w.click('next');
    }
    seen.push(w.visible()[0]);
    assert.deepEqual(seen, ['earlier', 'dock', 'focus', 'pet', 'cli', 'done']);
  });

  test('step 1 shows the move, Confirm runs that preview once, and a change re-previews', async () => {
    let found = true;
    let n = 0;
    const answers = [];
    const w = await load((cmd, args) => {
      if (cmd === 'setup_status') return status({ migration: { found, agents: found ? ['local.bellwether.brain (running)'] : [] } });
      if (cmd === 'migration_preview') return { found: true, planId: `move-${++n}`, text: `1. The hook token\n   - copy the token (preview ${n})` };
      if (cmd === 'migration_apply') return answers.shift()(args);
      return {};
    });
    assert.deepEqual(w.calls.map((c) => c[0]), ['setup_status', 'migration_preview'], 'the preview is shown at once');
    assert.equal(w.el('earlier-plan').hidden, false);
    assert.match(w.el('earlier-plan').textContent, /preview 1/);
    assert.equal(w.el('earlier-apply').disabled, false);
    // Something changed: nothing done, the fresh preview shown, Confirm on it again.
    answers.push(() => ({ outcome: 'changed', message: 'something changed since the preview', preview: { found: true, planId: 'move-9', text: 'fresh' } }));
    await w.click('earlier-apply');
    assert.deepEqual(w.calls.at(-1), ['migration_apply', { planId: 'move-1' }]);
    assert.match(w.el('earlier-result').textContent, /changed since the preview/);
    assert.equal(w.el('earlier-plan').textContent, 'fresh');
    // Applied: the result shown, the steps stay without login and hooks.
    answers.push(() => {
      found = false;
      return { outcome: 'applied', text: '4. Start at login\n   the new brain answers on port 47431' };
    });
    await w.click('earlier-apply');
    assert.deepEqual(w.calls.at(-2), ['migration_apply', { planId: 'move-9' }]);
    assert.match(w.el('earlier-result').textContent, /the new brain answers/);
    assert.equal(w.el('earlier-result').classList.contains('error'), false);
    assert.equal(w.el('earlier-apply').disabled, true, 'a used preview is gone');
    assert.equal(w.el('earlier-text').textContent, 'Moved into the app.');
    assert.equal(w.el('progress').textContent, 'Step 1 of 6');
    // A failed move (undone) is shown as an error.
    const w2 = await load((cmd) => (cmd === 'setup_status' ? status({ migration: { found: true, agents: [] } }) : cmd === 'migration_preview' ? { found: true, planId: 'm', text: 't' } : { outcome: 'failed', text: 'the move to the app\'s login items didn\'t work: …' }));
    assert.match(w2.el('earlier-text').textContent, /hook entries/);
    await w2.click('earlier-apply');
    assert.equal(w2.el('earlier-result').classList.contains('error'), true);
  });

  test('Install applies the previewed plan by its id; a file changed since the preview shows the refusal and the fresh preview', async () => {
    let applied = 0;
    const preview = (id, text) => ({ action: 'install', planId: id, changed: true, diffText: text, summary: 'bellwether matcher groups 0 -> 16 (16 events subscribed); every other line of the file is unchanged.', othersUntouched: 2, warnings: [], newToken: true, layoutKept: true });
    const w = await load((cmd, args) => {
      if (cmd === 'setup_status') return status();
      if (cmd === 'hooks_preview') return preview('plan-1-aaaa', '--- a\n+++ b\n+"X-Bellwether-Token": "abcd…"');
      if (cmd === 'hooks_apply') {
        applied++;
        if (args.planId === 'plan-1-aaaa') return { outcome: 'changed', message: '/Users/new/.claude/settings.json changed since the preview, so nothing was written. Here is a fresh preview; check it and confirm again.', preview: preview('plan-2-bbbb', '--- a\n+++ b\n+fresh') };
        return { outcome: 'applied', summary: 'written: bellwether matcher groups 0 -> 16', backup: null, warnings: [] };
      }
      return {};
    });
    await w.click('next');
    assert.deepEqual(w.visible(), ['hooks']);
    await w.click('hooks-preview-install');
    assert.deepEqual(w.calls.at(-1), ['hooks_preview', { action: 'install' }]);
    assert.equal(w.el('hooks-plan').hidden, false);
    assert.match(w.el('hooks-diff').textContent, /abcd…/);
    assert.match(w.el('hooks-summary').textContent, /Other tools' hooks left untouched: 2\..*new hook token/);
    await w.click('hooks-apply');
    assert.deepEqual(w.calls.find((c) => c[0] === 'hooks_apply'), ['hooks_apply', { planId: 'plan-1-aaaa' }]);
    assert.match(w.el('hooks-result').textContent, /changed since the preview/);
    assert.ok(w.el('hooks-result').classList.contains('error'));
    assert.equal(w.el('hooks-diff').textContent, '--- a\n+++ b\n+fresh', 'the fresh preview is shown');
    await w.click('hooks-apply');
    assert.deepEqual(w.calls.filter((c) => c[0] === 'hooks_apply').at(-1), ['hooks_apply', { planId: 'plan-2-bbbb' }]);
    assert.equal(applied, 2);
    assert.match(w.el('hooks-result').textContent, /^written: /);
    assert.equal(w.el('hooks-plan').hidden, true);
  });

  test('a refused or failed Install drops the used plan: Install is off until a fresh preview arrives', async () => {
    let fail = false;
    let applyCalls = 0;
    const preview = (id) => ({ action: 'install', planId: id, changed: true, diffText: `diff ${id}`, summary: 's', othersUntouched: 0, warnings: [], newToken: false, layoutKept: true });
    let pending;
    const w = await load((cmd) => {
      if (cmd === 'setup_status') return status();
      if (cmd === 'hooks_preview') return preview('plan-1');
      if (cmd === 'hooks_apply') {
        applyCalls++;
        if (fail) throw 'cannot write the file';
        // Held open, to look at the page while the answer is on its way.
        return new Promise((resolve) => { pending = resolve; });
      }
      return {};
    });
    await w.click('next');
    await w.click('hooks-preview-install');
    assert.equal(w.el('hooks-apply').disabled, false);
    const clicking = w.el('hooks-apply').click();
    await settle();
    assert.equal(w.el('hooks-apply').disabled, true, 'off while the answer is on its way');
    assert.equal(w.el('hooks-plan').hidden, true, 'the stale diff is gone');
    pending({ outcome: 'changed', message: 'changed since the preview', preview: preview('plan-2') });
    await clicking;
    await settle();
    assert.equal(w.el('hooks-diff').textContent, 'diff plan-2');
    assert.equal(w.el('hooks-apply').disabled, false, 'the fresh preview can be installed');
    // An error: no plan left, Install off, until Show the changes again.
    fail = true;
    await w.click('hooks-apply');
    assert.match(w.el('hooks-result').textContent, /cannot write the file/);
    assert.equal(w.el('hooks-apply').disabled, true);
    assert.equal(w.el('hooks-plan').hidden, true);
    await w.click('hooks-apply');
    assert.equal(applyCalls, 2, 'a used plan is never sent again');
    fail = false;
    await w.click('hooks-preview-install');
    assert.equal(w.el('hooks-apply').disabled, false);
  });

  test('a refused Import… shows the refusal; refusing a permission shows its words, and nothing else changes', async () => {
    const w = await load((cmd, args) => {
      if (cmd === 'setup_status') return status();
      if (cmd === 'pet_import') throw '/Users/new/evil.xml was refused: it declares a DOCTYPE or ENTITY, which pet files never need. Nothing was imported.';
      if (cmd === 'permission_request') return { ok: false, message: `Automation for ${args.kind === 'automation-iterm2' ? 'iTerm2' : 'Terminal'} was refused (-1743)` };
      return {};
    });
    for (let i = 0; i < 4; i++) await w.click('next');
    assert.deepEqual(w.visible(), ['pet']);
    assert.match(w.el('pet-intro').textContent, /No pet is installed yet/);
    await w.click('pet-import');
    assert.match(w.el('pet-result').textContent, /DOCTYPE or ENTITY.*Nothing was imported/);
    assert.ok(w.el('pet-result').classList.contains('error'));
    await w.click('back');
    await w.click('focus-terminal');
    assert.deepEqual(w.calls.filter((c) => c[0] === 'permission_request').at(-1), ['permission_request', { kind: 'automation-terminal' }]);
    assert.match(w.el('focus-result').textContent, /refused \(-1743\)/);
  });

  test('pets are listed with their still and chosen by name', async () => {
    const tiny = { name: 'tiny', kind: 'imported', title: 'Tiny', still: { png: 'AAAA', tilesx: 2, tilesy: 1, frame: 1, transparency: 'Magenta' } };
    const w = await load((cmd, args) => {
      if (cmd === 'setup_status') return status({ pets: { current: 'bellwether', list: [tiny], editable: true } });
      if (cmd === 'pet_select') return { current: args.name, pets: { current: args.name, list: [tiny], editable: true } };
      return {};
    });
    for (let i = 0; i < 4; i++) await w.click('next');
    const item = w.el('pet-list').children[0];
    assert.equal(item.children[0].tag, 'canvas', 'a still of its first frame');
    assert.match(item.text(), /Tiny \(tiny\)/);
    const use = item.children.find((c) => c.tag === 'button');
    await use.click();
    await settle();
    assert.deepEqual(w.calls.at(-1), ['pet_select', { name: 'tiny' }]);
  });
});
