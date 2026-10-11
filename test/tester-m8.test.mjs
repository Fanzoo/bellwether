import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { describe, test } from 'node:test';
import { checkLocatorLog } from '../brain/doctor.mjs';
import { AGENT_DIR, tempDir } from './helpers.mjs';

/**
 * Tester probes for Milestone 8. Nothing here sends AppleScript, runs launchctl or
 * focuses anything: the locator is exercised through the shell's `--locate` debug flag
 * with AppleScript switched off and a fake `herdr`, and the rest is static or doctor-side.
 */

const SRC = path.join(AGENT_DIR, 'shell', 'src-tauri', 'src');
const locatorRs = fs.readFileSync(path.join(SRC, 'locator.rs'), 'utf8');
const code = locatorRs.slice(0, locatorRs.indexOf('#[cfg(test)]')).replace(/\/\/.*$/gm, '');

describe('locator.rs: nothing from a header reaches a shell or a script unvalidated', () => {
  test('no shell is ever used: no sh/bash/zsh, no `-c`, no system(), only argv-style Command', () => {
    assert.doesNotMatch(code, /["'](\/bin\/)?(ba|z|da)?sh["']|"-c"|system\(|popen|Command::new\(&?format|\.arg\(&?format!\("\{/);
    assert.equal([...code.matchAll(/Command::new\(/g)].length, 1, 'exactly one place spawns a process (RealRunner)');
    assert.match(code, /cmd\.args\(args\)/);
  });
  test('osascript runs only with a fixed `-e <script>` argv, and scripts are built from validated values only', () => {
    assert.equal([...code.matchAll(/run\("\/usr\/bin\/osascript"/g)].length, 1);
    // every {placeholder} inside a script format string is the validated uuid or tty, nothing else
    const scripts = [...code.matchAll(/format!\(\s*"tell application[\s\S]*?"\s*\)/g)].map((m) => m[0]);
    assert.ok(scripts.length >= 3);
    for (const s of scripts) for (const ph of s.matchAll(/\{(\w+)\}/g)) assert.ok(['uuid', 'tty', 'act'].includes(ph[1]), `script placeholder {${ph[1]}}`);
    assert.match(code, /uuid\.len\(\) >= 8 && uuid\.chars\(\)\.all\(\|c\| c\.is_ascii_hexdigit\(\) \|\| c == '-'\)/);
    assert.match(code, /fn safe_tty[\s\S]*?is_ascii_alphanumeric\(\)[\s\S]*?"-"\.into\(\)/);
    // ttys come from ps, not headers
    assert.doesNotMatch(code, /fn (iterm|terminal)_script[^{]*\(\s*[^)]*header/i);
  });
  test('`open` is only ever called as `open -a <app>`, with app names from a fixed table or a ps path ending in .app', () => {
    for (const m of code.matchAll(/run\("\/usr\/bin\/open",\s*&\[([^\]]*)\]/g)) assert.match(m[1], /^"-a",/, m[0]);
  });
  test('herdr arguments from headers are validated before use (pane id and socket path)', () => {
    // Today the pane header is passed straight to `herdr pane get`; a value starting with "-" is read by herdr as an option.
    const paneUse = /let pane = t\.herdr_pane\.as_deref\(\)\?;/.exec(code);
    assert.ok(paneUse);
    const after = code.slice(paneUse.index, paneUse.index + 400);
    assert.match(after, /starts_with\('-'\)|is_ascii_alphanumeric|valid_pane|!pane\.chars\(\)\.all/, 'the herdr pane header is not validated (a value like --help becomes a herdr option)');
  });
});

describe('--locate with hostile terminal values (real binary, AppleScript off, fake herdr)', () => {
  const bin = path.join(AGENT_DIR, 'shell', 'src-tauri', 'target', 'release', 'bellwether-shell');
  const skip = !fs.existsSync(bin) || process.platform !== 'darwin';
  const setup = () => {
    const root = tempDir('m8-');
    const log = path.join(root, 'fake.log');
    const fake = path.join(root, 'fakeherdr');
    fs.writeFileSync(fake, `#!/bin/sh\n{ echo "ARGC=$#"; for a in "$@"; do printf 'ARG<%s>\\n' "$a" | head -c 300; done; echo "SOCK<$HERDR_SOCKET_PATH>"; echo ---; } >> "${log}"\necho '{"id":"x","error":{"code":"pane_not_found"}}'\n`, { mode: 0o755 });
    const cfg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
    cfg.locator.appleScript = false;
    cfg.locator.herdrPath = fake;
    const cfgFile = path.join(root, 'cfg.json');
    fs.writeFileSync(cfgFile, JSON.stringify(cfg));
    const locate = (terminal) => spawnSync(bin, ['--locate', JSON.stringify(terminal)], { encoding: 'utf8', timeout: 30000, env: { PATH: process.env.PATH, HOME: root, BELLWETHER_AGENT_DIR: AGENT_DIR, BELLWETHER_DATA_DIR: root, BELLWETHER_CONFIG: cfgFile, BELLWETHER_HOOK_TOKEN: 'm8TestTokenNotReal0123456789' } });
    const calls = () => (fs.existsSync(log) ? fs.readFileSync(log, 'utf8') : '');
    return { root, locate, calls };
  };

  test('hostile pane ids and socket paths never reach herdr; nothing is executed; nothing crashes; the chain still falls back', { skip }, () => {
    const { root, locate, calls } = setup();
    const sentinel = path.join(root, 'PWNED');
    const hostile = [`w1:p1; touch ${sentinel}`, `$(touch ${sentinel})`, `\`touch ${sentinel}\``, `x\ntouch ${sentinel}`, `a" & do shell script "touch ${sentinel}" & "`, '--help', '-h', '-x', 'ünï😀‮', 'A'.repeat(100_000), 'a b', '', 'w1:p1/../x', 'A'.repeat(65)];
    for (const h of hostile.filter((x) => x !== '')) {
      const r = locate({ herdrPane: h, herdrSocket: `/tmp/x${h.slice(0, 12)}`, itermSession: h, termSession: h, program: 'iTerm.app' });
      assert.equal(r.status, 0, `${h.slice(0, 30)}: ${r.stderr.slice(-200)}`);
      assert.match(r.stdout + r.stderr, /locate:/, 'the lookup still answers');
    }
    assert.equal(fs.existsSync(sentinel), false, 'a header value was executed');
    assert.equal(calls(), '', 'a hostile value reached herdr');
  });

  test('socket paths: a symlink, a plain file, a relative path, `..`, a control character, an over-long and a missing path never reach herdr; a real socket of mine does', { skip }, async () => {
    const net = await import('node:net');
    const { root, locate, calls } = setup();
    const sock = path.join('/tmp', `bk3-${process.pid}.sock`);
    const server = net.createServer(() => {}).listen(sock);
    await new Promise((r) => server.once('listening', r));
    try {
      const link = path.join(root, 'link.sock');
      fs.symlinkSync(sock, link);
      const plain = path.join(root, 'plain.sock');
      fs.writeFileSync(plain, 'x');
      for (const bad of [link, plain, 'relative.sock', `/tmp/../tmp/${path.basename(sock)}`, `${sock}\u0007`, `/tmp/${'a'.repeat(1100)}`, '/tmp/definitely-missing-bk3.sock', '/']) {
        const r = locate({ herdrPane: 'w1:p1', herdrSocket: bad, program: 'iTerm.app' });
        assert.equal(r.status, 0, bad.slice(0, 40));
      }
      assert.equal(calls(), '', 'a bad socket path reached herdr');
      const ok = locate({ herdrPane: 'w1:p1', herdrSocket: sock, program: 'iTerm.app' });
      assert.equal(ok.status, 0);
      const seen = calls();
      assert.equal([...seen.matchAll(/ARGC=3/g)].length, 1, 'a valid id and socket arrive as `herdr pane get <id>` with exactly 3 argv elements');
      assert.match(seen, /ARG<pane>\nARG<get>\nARG<w1:p1>\nSOCK<\/tmp\/bk3-\d+\.sock>/);
    } finally {
      server.close();
      fs.rmSync(sock, { force: true });
    }
  });

  test('valid ids of every allowed shape (and the 64-character limit) reach herdr as one argv element', { skip }, () => {
    const { locate, calls } = setup();
    const good = ['w1:p1', 'w4:p2', 'abc_DEF-1.2:3', 'a'.repeat(64)];
    for (const g of good) assert.equal(locate({ herdrPane: g, program: 'iTerm.app' }).status, 0);
    const seen = calls();
    assert.equal([...seen.matchAll(/ARGC=3/g)].length, good.length);
    for (const g of good) assert.ok(seen.includes(`ARG<${g}>`), g);
  });
});

describe('the normaliser drops bad herdr values before they reach state', () => {
  const sock = path.join('/tmp', `bk3n-${process.pid}.sock`);
  const norm = async (headers) => {
    const { normalise } = await import('../brain/normalise.mjs');
    return normalise({ hook_event_name: 'Stop', session_id: 's' }, headers, 1).terminal;
  };
  test('bad panes and sockets are dropped, not truncated; good ones are kept', async () => {
    for (const bad of ['--help', '-x', 'a b', ';id', 'x\ny', 'A'.repeat(65), 'w1:p1/../x', '$(id)']) assert.equal((await norm({ 'x-bellwether-herdr-pane': bad })).herdrPane, undefined, bad.slice(0, 20));
    for (const bad of ['relative.sock', '/tmp/../etc/x', `/tmp/${'a'.repeat(1100)}`, '/tmp/x\u0007y', '/tmp/a b/../c']) assert.equal((await norm({ 'x-bellwether-herdr-socket': bad })).herdrSocket, undefined, bad.slice(0, 30));
    const t = await norm({ 'x-bellwether-herdr-pane': 'w4:p2', 'x-bellwether-herdr-socket': '/Users/me/.config/herdr/herdr.sock' });
    assert.equal(t.herdrPane, 'w4:p2');
    assert.equal(t.herdrSocket, '/Users/me/.config/herdr/herdr.sock');
    void sock;
  });
  test('the identity key never contains a dropped value', async () => {
    const { normalise } = await import('../brain/normalise.mjs');
    const { identityOf } = await import('../brain/identity.mjs');
    const ev = normalise({ hook_event_name: 'Stop', session_id: 's' }, { 'x-bellwether-herdr-pane': '--help', 'x-bellwether-herdr-socket': '/tmp/../x' }, 1);
    assert.equal(identityOf(ev).key, 'session:s');
  });
});

describe('doctor: automation and herdr CLI checks', () => {
  const run = (text) => {
    const d = tempDir('m8-doc-');
    fs.writeFileSync(path.join(d, 'shell.log'), text);
    return checkLocatorLog(d);
  };
  test('refusals and a missing herdr are named; only the current run counts; a clean log gives nothing', () => {
    const r = run('1 bellwether-shell: starting: x\n2 bellwether-shell: locator: Automation for iTerm2 was refused (a)\n3 bellwether-shell: locator: herdr CLI not found at "/nope/herdr" (locator.herdrPath); x\n');
    assert.deepEqual(r.map((c) => c.name), ['automation', 'herdr CLI']);
    assert.match(r[0].detail, /iTerm2/);
    assert.deepEqual(run('1 bellwether-shell: starting: x\n2 bellwether-shell: locator: Automation for iTerm2 was refused (a)\n3 bellwether-shell: starting: again\n'), []);
    assert.deepEqual(checkLocatorLog(path.join(os.tmpdir(), 'no-such-dir-m8')), []);
  });
  test('a webview line cannot fake a warning, a fake restart line cannot hide a real one, and a malformed prefix is ignored', () => {
    const real = '1 bellwether-shell: starting: x\n2 bellwether-shell: locator: Automation for iTerm2 was refused (a)\n';
    assert.equal(run(real + '3 bellwether-shell: webview: starting: fake restart\n').length, 1, 'a webview line hid a real warning');
    assert.deepEqual(run('1 bellwether-shell: starting: x\n2 evil bellwether-shell: locator: Automation for Terminal was refused\n'), []);
    assert.deepEqual(run('1 bellwether-shell: starting: x\n2 bellwether-shell:   webview: locator: Automation for iTerm2 was refused\n'), []);
    assert.deepEqual(run('1 bellwether-shell: starting: x\n2 bellwether-shell: webview: locator: herdr CLI not found at "/x" (y)\n'), []);
  });
  test('a line the webview wrote through shell_log cannot fake a locator warning', () => {
    const r = run('1 bellwether-shell: starting: x\n2 bellwether-shell: webview: locator: Automation for iTerm2 was refused (a pet title can say this)\n');
    assert.deepEqual(r, [], 'doctor treated a webview log line as a locator refusal');
  });
});
