import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { withDefaults } from '../brain/defaults.mjs';
import { checkLocatorLog } from '../brain/doctor.mjs';
import { Flock } from '../brain/flock.mjs';
import { AGENT_DIR, tempDir } from './helpers.mjs';

/** Milestone 8, the parts outside Rust (the Rust side: cargo test, locator.rs and placement.rs). */

const RS = path.join(AGENT_DIR, 'shell', 'src-tauri', 'src');
const read = (f) => fs.readFileSync(path.join(RS, f), 'utf8');

describe('doctor reports what the locator ran into', () => {
  const log = (lines) => {
    const d = tempDir();
    fs.writeFileSync(path.join(d, 'shell.log'), lines.join('\n') + '\n');
    return d;
  };
  test('Automation refused names the app and where to allow it; only since the last start', () => {
    const d = log([
      '1 bellwether-shell: starting: agent dir x',
      '2 bellwether-shell: locator: Automation for iTerm2 was refused (System Settings > Privacy & Security > Automation); …',
      '3 bellwether-shell: starting: agent dir x',
      '4 bellwether-shell: locator: Automation for Terminal was refused (System Settings > Privacy & Security > Automation); …',
    ]);
    const [c] = checkLocatorLog(d);
    assert.equal(c.name, 'automation');
    assert.equal(c.status, 'WARN');
    assert.match(c.detail, /Automation for Terminal/);
    assert.doesNotMatch(c.detail, /iTerm2/, 'an earlier run does not count');
    assert.match(c.detail, /System Settings > Privacy & Security > Automation/);
  });
  test('a missing herdr CLI is reported; no log, or a clean log, reports nothing', () => {
    const d = log(['1 bellwether-shell: starting: x', '2 bellwether-shell: locator: herdr CLI not found at "/nope/herdr" (locator.herdrPath); …']);
    assert.match(checkLocatorLog(d)[0].detail, /"\/nope\/herdr"/);
    assert.deepEqual(checkLocatorLog(tempDir()), []);
    assert.deepEqual(checkLocatorLog(log(['1 bellwether-shell: starting: x', '2 bellwether-shell: placement: a -> "Monitor #1" (herdr, Exact)'])), []);
  });
});

describe('per-monitor slots through the brain', () => {
  test('the snapshot carries each identity\'s slots, and `placed` adds one for another monitor', () => {
    const f = new Flock({ config: withDefaults({ port: 0 }), t: 0 });
    f.apply({ receivedAt: 1, event: 'UserPromptSubmit', sessionId: 's', cwd: '/w/p', projectLabel: 'p', terminal: { herdrPane: 'w1:p1', herdrSocket: '/s' } });
    const s = f.snapshot().flock[0];
    assert.deepEqual(s.slots, { primary: s.slot });
    assert.deepEqual(f.control({ action: 'placed', identityKey: s.identityKey, monitor: 'Monitor #2', slot: 3 }, 2), { ok: true });
    assert.deepEqual(f.snapshot().flock[0].slots, { primary: s.slot, 'Monitor #2': 3 });
  });
});

describe('the shell side, by its source', () => {
  test('still six commands; a click focuses through the locator on its own thread; each strip sees its own monitor', () => {
    const lib = read('lib.rs');
    // The strip's six first; the rest are the onboarding window's (M7, onboarding.rs).
    assert.match(lib, /generate_handler!\[\s*get_pet,\s*get_snapshot,\s*report_hitboxes,\s*sprite_clicked,\s*shell_log,\s*debug_snapshot,\s*onboarding::/);
    const handler = /generate_handler!\[([^\]]+)\]/.exec(lib)[1].split(',').map((s) => s.trim()).filter(Boolean);
    assert.equal(handler.filter((c) => !c.startsWith('onboarding::')).length, 6);
    assert.match(lib, /fn sprite_clicked\([^)]*locator: tauri::State<'_, Arc<locator::Locator>>\)/);
    assert.match(lib, /std::thread::spawn\(move \|\| \{\s*let _guard = guard;\s*let result = locator\.focus\(&terminal\);/);
    assert.match(lib, /placement::view_for\(&s, &mine, &p\.primary/);
  });
  test('AppleScript only with a hex UUID or a ps tty, only to a running app, and never when locator.appleScript is false', () => {
    const loc = read('locator.rs');
    assert.match(loc, /c\.is_ascii_hexdigit\(\) \|\| c == '-'/);
    assert.match(loc, /fn safe_tty/);
    assert.match(loc, /if !self\.cfg\.apple_script \|\| self\.denied/);
    assert.match(loc, /`tell application` would launch an app that isn't running/);
  });
  test('config.example.json lists the built strategies and the AppleScript switch', () => {
    const c = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
    assert.ok(c.locator.strategies.includes('process-tree'));
    assert.equal(c.locator.appleScript, true);
  });
});

describe('herdr header values are checked at the door (verification M8 D1)', () => {
  test('pane ids like herdr makes them, sockets as absolute paths; anything else never reaches state', async () => {
    const { terminalFrom, HERDR_PANE_RE } = await import('../brain/normalise.mjs');
    const t = (pane, socket) => terminalFrom({ 'x-bellwether-herdr-pane': pane, 'x-bellwether-herdr-socket': socket, 'x-bellwether-term-program': 'iTerm.app' });
    assert.deepEqual(t('w4:p2', '/Users/x/.config/herdr/herdr.sock'), { herdrPane: 'w4:p2', herdrSocket: '/Users/x/.config/herdr/herdr.sock', program: 'iTerm.app' });
    for (const bad of ['--help', '-x', 'w1 p1', 'w1:p1;rm', 'a'.repeat(65), 'w1:p1\n']) assert.equal(t(bad, '/s').herdrPane, undefined, JSON.stringify(bad));
    for (const bad of ['relative/h.sock', '/tmp/../etc/x', '/a\u0000b', '/' + 'a'.repeat(1100)]) assert.equal(t('w1:p1', bad).herdrSocket, undefined, JSON.stringify(bad.slice(0, 20)));
    assert.ok(HERDR_PANE_RE.test('w1') && !HERDR_PANE_RE.test(''));
  });
  test('doctor only believes lines the shell wrote itself (verification M8 D3)', () => {
    const d = tempDir();
    fs.writeFileSync(path.join(d, 'shell.log'), [
      '1 bellwether-shell: starting: x',
      '2 bellwether-shell: webview: locator: Automation for iTerm2 was refused (a pet title)',
      '3 bellwether-shell: webview: x bellwether-shell: locator: herdr CLI not found at "/x" (y)',
      '4 bellwether-shell: webview: bellwether-shell: starting: hides the real lines before it',
    ].join('\n') + '\n');
    assert.deepEqual(checkLocatorLog(d), []);
    fs.appendFileSync(path.join(d, 'shell.log'), '5 bellwether-shell: locator: Automation for Terminal was refused (z)\n');
    assert.deepEqual(checkLocatorLog(d).map((c) => c.name), ['automation']);
    // A fake "starting:" from the webview doesn't hide an earlier real refusal.
    const e = tempDir();
    fs.writeFileSync(path.join(e, 'shell.log'), '1 bellwether-shell: starting: x\n2 bellwether-shell: locator: Automation for iTerm2 was refused (a)\n3 bellwether-shell: webview: bellwether-shell: starting: fake\n');
    assert.deepEqual(checkLocatorLog(e).map((c) => c.name), ['automation']);
  });
});

describe('M8-14: one focus at a time, released even if focus panics', () => {
  test('the busy flag is cleared by a guard dropped in the focus thread, not by a line after focus()', () => {
    const lib = read('lib.rs');
    assert.match(lib, /impl Drop for Focusing \{\s*fn drop\(&mut self\) \{\s*FOCUSING\.store\(false/);
    assert.match(lib, /let _guard = guard;\s*let result = locator\.focus\(&terminal\);/);
  });
});
