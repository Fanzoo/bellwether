import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { AGENT_DIR, FIXTURES, TOKEN, readInstallerVectors, readVectors, spawnBrainSync, tempDir } from './helpers.mjs';
import { ISOLATED } from './setup.mjs';
import { EVENTS, HTTP_UNSUPPORTED_EVENTS, checkSettings, countGroups, deepEqual, diffStats, hookBlock, install, isBellwetherHandler, planEdit, unifiedDiff, uninstall } from './impl.mjs';

/**
 * The installer. Every test works on a temp copy of an invented settings file shaped
 * like a real user's (another tool's hooks, afplay sounds, herdr, a statusLine, unknown keys). The real
 * ~/.claude/settings.json is never passed to anything here.
 *
 * The installer's parts come from the Rust brain (test/impl.mjs) and the CLI is the
 * binary (distribution plan, milestone 4).
 */

// The configured port: in the test run, the suite's own (test/setup.mjs), never the live 47417.
const PORT = JSON.parse(fs.readFileSync(ISOLATED.BELLWETHER_CONFIG, 'utf8')).port;
const SAFE_HOME = tempDir('bellwether-home-');
const fixture = () => JSON.parse(fs.readFileSync(path.join(FIXTURES, 'settings-sample.json'), 'utf8'));

function copyFixture() {
  const dir = tempDir();
  const file = path.join(dir, 'settings.json');
  fs.copyFileSync(path.join(FIXTURES, 'settings-sample.json'), file);
  return { dir, file };
}

/** Runs the real CLI with --settings, a test token and a temp data dir. */
function cli(args, dataDir) {
  const r = spawnBrainSync([...args], {
    cwd: AGENT_DIR,
    encoding: 'utf8',
    // HOME points at a temp dir, so even a regression that fell back to the default
    // settings path could not reach the real ~/.claude/settings.json.
    env: { ...process.env, BELLWETHER_HOOK_TOKEN: TOKEN, BELLWETHER_DATA_DIR: dataDir, HOME: SAFE_HOME, USERPROFILE: SAFE_HOME, BELLWETHER_CWD: '' },
  });
  return { code: r.status, stdout: r.stdout, stderr: r.stderr };
}

describe('hook block', () => {
  test('one entry per subscribed event, no matcher, timeout 1, the token as a literal header', () => {
    const block = hookBlock(PORT, TOKEN);
    assert.deepEqual(Object.keys(block.hooks), EVENTS);
    assert.equal(EVENTS.length, 16);
    for (const groups of Object.values(block.hooks)) {
      assert.equal(groups.length, 1);
      assert.equal(groups[0].matcher, undefined);
      const [h] = groups[0].hooks;
      assert.equal(h.type, 'http');
      assert.equal(h.url, `http://127.0.0.1:${PORT}/hook`);
      assert.equal(h.timeout, 1);
      assert.equal(h.headers['X-Bellwether-Token'], TOKEN);
      for (const [name, value] of Object.entries(h.headers)) {
        if (name === 'X-Bellwether-Token') continue;
        assert.ok(h.allowedEnvVars.includes(value.slice(1)), `${name} interpolates an allowed variable`);
      }
    }
  });

  test('no subscribed event is one where Claude Code skips HTTP hooks (SessionStart, Setup)', () => {
    for (const e of HTTP_UNSUPPORTED_EVENTS) assert.ok(!EVENTS.includes(e), e);
  });

  test('deliberately unsubscribed events stay out', () => {
    for (const e of ['MessageDisplay', 'PostToolBatch', 'UserPromptExpansion', 'PreModelSwitch', 'TeammateIdle', 'TaskCreated', 'TaskCompleted']) {
      assert.ok(!EVENTS.includes(e), e);
    }
  });

  test('ownership is by URL prefix only', () => {
    assert.ok(isBellwetherHandler({ type: 'http', url: `http://127.0.0.1:${PORT}/hook` }, PORT));
    assert.ok(!isBellwetherHandler({ type: 'http', url: 'http://127.0.0.1:47418/hook' }, PORT));
    assert.ok(!isBellwetherHandler({ type: 'http', url: `http://localhost:${PORT}/hook` }, PORT));
    assert.ok(!isBellwetherHandler({ type: 'command', command: `curl http://127.0.0.1:${PORT}/hook` }, PORT));
  });
});

describe('install / uninstall', () => {
  test('install adds 16 groups and leaves every other entry as it was, in order', () => {
    const before = fixture();
    const after = install(before, PORT, TOKEN);
    assert.equal(Object.values(countGroups(after, PORT)).reduce((a, b) => a + b), 16);
    for (const [event, groups] of Object.entries(before.hooks)) {
      assert.deepEqual(after.hooks[event].slice(0, groups.length), groups, `${event}: existing groups first and unchanged`);
      assert.equal(after.hooks[event].length, groups.length + (EVENTS.includes(event) ? 1 : 0), event);
    }
    for (const k of Object.keys(before)) if (k !== 'hooks') assert.deepEqual(after[k], before[k], k);
  });

  test('install removes a bellwether entry left on an event it no longer subscribes to', () => {
    const stale = structuredClone(fixture());
    stale.hooks.SessionStart.push({ hooks: [{ type: 'http', url: `http://127.0.0.1:${PORT}/hook`, timeout: 1 }] });
    const after = install(stale, PORT, TOKEN);
    assert.deepEqual(after.hooks.SessionStart, fixture().hooks.SessionStart);
  });

  test('install is idempotent', () => {
    const once = install(fixture(), PORT, TOKEN);
    assert.ok(deepEqual(install(once, PORT, TOKEN), once));
  });

  test('a changed token replaces the old entry rather than adding a second', () => {
    const old = install(fixture(), PORT, 'oldTokenOldTokenOld1');
    const fresh = install(old, PORT, TOKEN);
    assert.deepEqual(countGroups(fresh, PORT), Object.fromEntries(EVENTS.map((e) => [e, 1])));
    assert.ok(deepEqual(fresh, install(fixture(), PORT, TOKEN)));
  });

  test('install twice, uninstall once: none left, and the result equals the original', () => {
    const twice = install(install(fixture(), PORT, TOKEN), PORT, TOKEN);
    const removed = uninstall(twice, PORT);
    assert.deepEqual(countGroups(removed, PORT), {});
    assert.ok(deepEqual(removed, fixture()));
  });

  test('uninstall on a file with no hooks key removes the key it created', () => {
    const bare = { theme: 'dark' };
    assert.deepEqual(uninstall(install(bare, PORT, TOKEN), PORT), bare);
  });

  test('uninstall keeps a foreign handler that shares a group with bellwether', () => {
    const mixed = { hooks: { Stop: [{ hooks: [{ type: 'command', command: 'echo hi' }, { type: 'http', url: `http://127.0.0.1:${PORT}/hook` }] }] } };
    assert.deepEqual(uninstall(mixed, PORT), { hooks: { Stop: [{ hooks: [{ type: 'command', command: 'echo hi' }] }] } });
  });

  test('the dry-run diff only adds lines', () => {
    const { file } = copyFixture();
    const plan = planEdit(file, 'install', { port: PORT, token: TOKEN });
    const stats = diffStats(plan.oldText, plan.newText);
    assert.equal(stats.removed, 0, `removed: ${stats.removedLines.join(' | ')}`);
    assert.ok(stats.added > 16 * 20);
    // The diff the preview is made with, against the shared vectors
    // (test/fixtures/diff-vectors.json): the text and the counts, exactly.
    for (const v of readVectors('diff')) {
      assert.equal(unifiedDiff(v.old, v.new, v.options), v.expected, v.name);
      const s = diffStats(v.old, v.new);
      assert.deepEqual({ added: s.added, removed: s.removed }, v.stats, v.name);
    }
    const sample = readInstallerVectors().cases.find((c) => c.group === 'sample');
    assert.equal(sample.settings, fs.readFileSync(path.join(FIXTURES, 'settings-sample.json'), 'utf8'), 'the sample vector is settings-sample.json');
    assert.equal(diffStats(sample.settings, sample.installed).removed, 0);
  });

  test('invalid JSON is refused with exit 3 and nothing is written', () => {
    const dir = tempDir();
    const file = path.join(dir, 'settings.json');
    fs.writeFileSync(file, '{ "hooks": { ,');
    const r = cli(['hooks', 'install', '--write', '--settings', file], path.join(dir, 'data'));
    assert.equal(r.code, 3, r.stderr);
    assert.match(r.stderr, /not valid JSON/);
    assert.equal(fs.readFileSync(file, 'utf8'), '{ "hooks": { ,');
  });
});

describe('the CLI against a temp copy', () => {
  test('dry run writes nothing; --write backs up, installs; second install is a no-op; uninstall restores', () => {
    const { dir, file } = copyFixture();
    const data = path.join(dir, 'data');
    const original = fs.readFileSync(file);
    const mtime = fs.statSync(file).mtimeMs;

    const dry = cli(['hooks', 'install', '--settings', file], data);
    assert.equal(dry.code, 0, dry.stderr);
    assert.match(dry.stdout, new RegExp(`^\\+\\s+"url": "http://127\\.0\\.0\\.1:${PORT}/hook",$`, 'm'));
    assert.doesNotMatch(dry.stdout, /^-(?!-- )/m, 'no removed lines');
    assert.match(dry.stderr, /Nothing was written/);
    assert.deepEqual(fs.readFileSync(file), original);
    assert.equal(fs.statSync(file).mtimeMs, mtime);

    const write = cli(['hooks', 'install', '--write', '--settings', file], data);
    assert.equal(write.code, 0, write.stderr);
    const backups = fs.readdirSync(path.join(data, 'backups'));
    assert.equal(backups.length, 1);
    assert.deepEqual(fs.readFileSync(path.join(data, 'backups', backups[0])), original, 'backup keeps the original bytes');
    const installed = JSON.parse(fs.readFileSync(file, 'utf8'));
    assert.equal(Object.keys(countGroups(installed, PORT)).length, 16);

    const again = cli(['hooks', 'install', '--write', '--settings', file], data);
    assert.equal(again.code, 0);
    assert.match(again.stderr, /already installed, no changes/);
    assert.equal(fs.readdirSync(path.join(data, 'backups')).length, 1, 'no write, no backup');

    const un = cli(['hooks', 'uninstall', '--write', '--settings', file], data);
    assert.equal(un.code, 0, un.stderr);
    assert.ok(deepEqual(JSON.parse(fs.readFileSync(file, 'utf8')), fixture()), 'semantically identical to the original');
  });

  test('hooks print emits a settings file with only bellwether hooks (on stdout the token is masked; --out keeps it)', () => {
    const r = cli(['hooks', 'print'], tempDir());
    assert.equal(r.code, 0, r.stderr);
    assert.ok(!r.stdout.includes(TOKEN), 'stdout shows the token');
    assert.deepEqual(JSON.parse(r.stdout), JSON.parse(JSON.stringify(hookBlock(PORT, TOKEN)).split(TOKEN).join(`${TOKEN.slice(0, 4)}…`)));
    const dir = tempDir();
    const out = path.join(dir, 'h.json');
    assert.equal(cli(['hooks', 'print', '--out', out], dir).code, 0);
    assert.deepEqual(JSON.parse(fs.readFileSync(out, 'utf8')), hookBlock(PORT, TOKEN));
  });

  test('without BELLWETHER_HOOK_TOKEN, install refuses with exit 3', () => {
    const { dir, file } = copyFixture();
    const r = spawnBrainSync(['hooks', 'install', '--settings', file], {
      cwd: AGENT_DIR,
      encoding: 'utf8',
      env: { ...process.env, BELLWETHER_HOOK_TOKEN: '', BELLWETHER_DATA_DIR: path.join(dir, 'data') },
    });
    assert.equal(r.status, 3);
    assert.match(r.stderr, /BELLWETHER_HOOK_TOKEN is not set/);
  });
});

describe('flags that take a value are never optional', () => {
  const homeSettings = path.join(SAFE_HOME, '.claude', 'settings.json');
  const original = '{\n  "theme": "dark"\n}\n';

  for (const argv of [
    ['hooks', 'install', '--write', '--settings'],
    ['hooks', 'install', '--settings', '--write'],
    ['hooks', 'install', '--write', '--settings='],
    ['hooks', 'install', '--write', '--settings', '  '],
    ['hooks', 'uninstall', '--write', '--settings'],
    ['hooks', 'install', '--write', '--setings', 'typo.json'],
    ['hooks', 'install', 'stray.json', '--write'],
    ['hooks', 'print', '--out'],
    ['doctor', '--settings'],
    ['serve', '--slow'],
    ['record', '--out'],
    ['serve', '--hang=yes'],
  ]) {
    test(`${argv.join(' ')}: exit 3, default settings untouched`, () => {
      fs.mkdirSync(path.dirname(homeSettings), { recursive: true });
      fs.writeFileSync(homeSettings, original);
      const r = cli(argv, path.join(tempDir(), 'data'));
      assert.equal(r.code, 3, r.stderr);
      assert.match(r.stderr, /needs a value|unknown option|unexpected argument|takes no value/);
      assert.equal(fs.readFileSync(homeSettings, 'utf8'), original);
    });
  }
});

describe('test/DIVERGENCES.md 21 and 22: wording and depth, each brain its own', () => {
  test('the "not laid out" summary names the backup folder, and a missing token names BELLWETHER_HOOK_TOKEN', () => {
    const dir = tempDir();
    const file = path.join(dir, 'settings.json');
    fs.writeFileSync(file, '{ "theme": "dark" }\n');
    const data = path.join(dir, 'data');
    const r = cli(['hooks', 'install', '--settings', file], data);
    assert.equal(r.code, 0, r.stderr);
    assert.ok(r.stderr.includes(`the backup in ${path.join(data, 'backups')}/ keeps them`), r.stderr);
    const none = spawnBrainSync(['hooks', 'print'], { encoding: 'utf8', env: { PATH: process.env.PATH, HOME: SAFE_HOME, BELLWETHER_DATA_DIR: path.join(tempDir(), 'd') } });
    assert.equal(none.status, 3);
    assert.match(none.stderr, /no hook token: BELLWETHER_HOOK_TOKEN is not set, and .*hook-token doesn't exist or is empty\. Make one with `bellwether token init`/);
  });

  test('a settings file nested 1100 deep: the brain refuses it (exit 3) and writes nothing (Node installed into it)', () => {
    const dir = tempDir();
    const file = path.join(dir, 'settings.json');
    const text = `{"deep": ${'['.repeat(1100)}${']'.repeat(1100)}}\n`;
    fs.writeFileSync(file, text);
    // The preview of 1100 levels, indented, is several MB.
    const p = spawnBrainSync(['hooks', 'install', '--write', '--settings', file], { cwd: AGENT_DIR, encoding: 'utf8', maxBuffer: 1 << 28, env: { ...process.env, BELLWETHER_HOOK_TOKEN: TOKEN, BELLWETHER_DATA_DIR: path.join(dir, 'data'), HOME: SAFE_HOME, USERPROFILE: SAFE_HOME, BELLWETHER_CWD: '' } });
    const r = { code: p.status, stderr: p.stderr };
    assert.equal(r.code, 3, r.stderr);
    assert.match(r.stderr, /is not valid JSON \(nested too deeply/);
    assert.equal(fs.readFileSync(file, 'utf8'), text);
  });
});

describe('a port is one spelling', () => {
  test('replay and doctor refuse --port spelled with a leading zero, a sign or spaces (exit 3), as hooks does', () => {
    for (const bad of ['047999', '+47999', ' 47999', '4.7999e4']) {
      for (const args of [['replay', '--burst', '10', '--seconds', '0.1', `--port=${bad}`], ['doctor', `--port=${bad}`]]) {
        const r = cli(args, path.join(tempDir(), 'data'));
        assert.equal(r.code, 3, `${args.join(' ')}: ${r.stderr}`);
      }
    }
  });
});

describe('files that hold the token are private', () => {
  test('hooks print --out creates missing directories and writes 0600', { skip: process.platform === 'win32' }, () => {
    const dir = tempDir();
    const out = path.join(dir, 'not', 'yet', 'hooks-only.json');
    const r = cli(['hooks', 'print', '--out', out], path.join(dir, 'data'));
    assert.equal(r.code, 0, r.stderr);
    assert.equal(fs.statSync(out).mode & 0o777, 0o600);
    assert.deepEqual(JSON.parse(fs.readFileSync(out, 'utf8')), hookBlock(PORT, TOKEN));
  });

  test('backups, a newly created settings file and the install record are 0600', { skip: process.platform === 'win32' }, () => {
    const { dir, file } = copyFixture();
    fs.chmodSync(file, 0o644);
    const data = path.join(dir, 'data');
    assert.equal(cli(['hooks', 'install', '--write', '--settings', file], data).code, 0);
    const [b] = fs.readdirSync(path.join(data, 'backups'));
    assert.equal(fs.statSync(path.join(data, 'backups', b)).mode & 0o777, 0o600);
    assert.equal(fs.statSync(path.join(data, 'installs.json')).mode & 0o777, 0o600);
    assert.equal(fs.statSync(file).mode & 0o777, 0o644, 'an existing file keeps its mode');
    const fresh = path.join(dir, 'fresh', 'settings.json');
    assert.equal(cli(['hooks', 'install', '--write', '--settings', fresh], data).code, 0);
    assert.equal(fs.statSync(fresh).mode & 0o777, 0o600);
  });
});

describe('layout and fidelity', () => {
  const write = (text) => {
    const dir = tempDir();
    const file = path.join(dir, 'settings.json');
    fs.writeFileSync(file, text);
    return { file, data: path.join(dir, 'data') };
  };
  const roundTrip = (text) => {
    const { file, data } = write(text);
    assert.equal(cli(['hooks', 'install', '--write', '--settings', file], data).code, 0);
    assert.equal(cli(['hooks', 'uninstall', '--write', '--settings', file], data).code, 0);
    return fs.readFileSync(file, 'utf8');
  };
  // The settings files are shared with the Rust port: test/fixtures/installer-vectors/,
  // each with the bytes after install, the preview, and the bytes after uninstall.
  const vectors = readInstallerVectors();
  const vectorPlan = (text, action) => {
    const { file, data } = write(text);
    const plan = planEdit(file, action, { port: vectors.port, token: vectors.token, dataDir: data });
    return { file, data, plan };
  };
  for (const v of vectors.cases.filter((c) => c.group === 'layout')) {
    test(`round trip is byte-identical: ${v.name}`, () => {
      assert.equal(roundTrip(v.settings), v.settings);
      assert.ok(v.roundTrip && v.uninstalled === v.settings);
      // In process, on the vectors' port: the installed bytes and the preview, exactly.
      const { plan } = vectorPlan(v.settings, 'install');
      assert.equal(plan.newText, v.installed);
      assert.equal(unifiedDiff(plan.oldText, plan.newText, { oldName: 'settings.json (current)', newName: 'settings.json (after install)' }), v.installDiff);
      assert.equal(plan.layoutKept, v.layoutKept);
    });
  }
  const base = { theme: 'dark', n: 1.5, hooks: { Stop: [{ hooks: [{ type: 'command', command: 'k' }] }] } };
  assert.equal(vectors.cases.find((c) => c.name === 'tabs').settings, JSON.stringify(base, null, '\t') + '\n', 'the tabs vector is this base');

  test('a file not in bellwether\'s layout says so in the preview instead of "every other line is unchanged"', () => {
    const v = vectors.cases.find((c) => c.group === 'reformatted');
    assert.equal(v.layoutKept, false);
    assert.equal(vectorPlan(v.settings, 'install').plan.newText, v.installed);
    const { file, data } = write(v.settings);
    const r = cli(['hooks', 'install', '--settings', file], data);
    assert.equal(r.code, 0, r.stderr);
    assert.match(r.stderr, /not laid out the way bellwether writes JSON/);
    assert.doesNotMatch(r.stderr, /every other line of the file is unchanged/);
  });

  test('a file in bellwether\'s layout says every other line is unchanged', () => {
    const { file, data } = write(JSON.stringify(base, null, 2) + '\n');
    assert.match(cli(['hooks', 'install', '--settings', file], data).stderr, /every other line of the file is unchanged/);
  });

  test('without an install record, uninstall still removes the event keys bellwether added', () => {
    const { file, data } = write('{\n  "theme": "dark"\n}\n');
    assert.equal(cli(['hooks', 'install', '--write', '--settings', file], data).code, 0);
    fs.rmSync(path.join(data, 'installs.json'));
    assert.equal(cli(['hooks', 'uninstall', '--write', '--settings', file], data).code, 0);
    assert.equal(fs.readFileSync(file, 'utf8'), '{\n  "theme": "dark"\n}\n');
  });

  test('a garbage group shape is refused with exit 3 naming the place, file untouched', () => {
    const text = '{\n  "hooks": {\n    "Stop": [\n      {\n        "hooks": "x"\n      }\n    ]\n  }\n}\n';
    const { file, data } = write(text);
    const r = cli(['hooks', 'install', '--write', '--settings', file], data);
    assert.equal(r.code, 3);
    assert.match(r.stderr, /hooks\.Stop\[0\]\.hooks" is not an array/);
    assert.equal(fs.readFileSync(file, 'utf8'), text);
  });
});

describe('every installer vector through the CLI (milestone 4: the Rust preview is Node\'s, byte for byte)', () => {
  const vectors = readInstallerVectors();
  // The CLI on the vectors' port: a config of its own, a temp data dir, the test token.
  const vectorCli = (args, dir) => {
    const cfg = path.join(dir, 'config.json');
    fs.writeFileSync(cfg, JSON.stringify({ port: vectors.port }));
    const r = spawnBrainSync(args, { cwd: AGENT_DIR, encoding: 'utf8', env: { ...process.env, BELLWETHER_CONFIG: cfg, BELLWETHER_HOOK_TOKEN: vectors.token, BELLWETHER_DATA_DIR: path.join(dir, 'data'), HOME: SAFE_HOME, BELLWETHER_CWD: '' } });
    return { code: r.status, stdout: r.stdout, stderr: r.stderr };
  };
  // The exotic vector is the one intended difference (test/DIVERGENCES.md): JSON.parse
  // moves its integer-like keys first, and the Rust brain keeps file order. Its preview and
  // bytes are pinned by crates/core/tests/hook_vectors.rs.
  const same = (v) => v.group !== 'exotic';
  for (const v of vectors.cases) {
    test(`hooks install previews ${v.name} as Node does; install then uninstall ${v.roundTrip ? 'gives back the original bytes' : "gives back Node's bytes"}`, () => {
      const dir = tempDir('bw-vec-');
      const file = path.join(dir, 'settings.json');
      fs.writeFileSync(file, v.settings);
      const masked = (t) => t.split(vectors.token).join(`${vectors.token.slice(0, 4)}…`);
      const dry = vectorCli(['hooks', 'install', '--settings', file], dir);
      assert.equal(dry.code, 0, dry.stderr);
      const preview = dry.stdout.split(`${file} (`).join('settings.json (');
      // The exotic vector under rust: Node's bytes with the three integer-like keys back in
      // file order (they are the settings' own lines 1-3), and the preview the brain's diff
      // makes of exactly those bytes (the diff itself is pinned by diff-vectors.json and the
      // frozen sample).
      const lines = (t) => t.split('\n');
      const rustInstalled = same(v) ? v.installed : [lines(v.installed)[0], ...lines(v.settings).slice(1, 4), ...lines(v.installed).slice(4)].join('\n');
      const rustPreview = same(v) ? v.installDiff : unifiedDiff(v.settings, rustInstalled, { oldName: 'settings.json (current)', newName: 'settings.json (after install)' });
      assert.equal(preview, masked(rustPreview), 'the preview, byte for byte (the token masked, as on any stdout)');
      assert.equal(fs.readFileSync(file, 'utf8'), v.settings, 'a dry run writes nothing');
      assert.equal(vectorCli(['hooks', 'install', '--write', '--settings', file], dir).code, 0);
      assert.equal(fs.readFileSync(file, 'utf8'), rustInstalled, 'the installed bytes');
      assert.equal(vectorCli(['hooks', 'uninstall', '--write', '--settings', file], dir).code, 0);
      const back = fs.readFileSync(file, 'utf8');
      if (v.roundTrip) assert.equal(back, v.settings);
      else if (same(v)) assert.equal(back, v.uninstalled);
      else {
        // The settings' bytes, with only the escaped string as JSON.stringify writes it
        // (Node's uninstalled line for it); the key order is the file's.
        const sLine = lines(v.uninstalled).find((l) => l.startsWith('  "s": '));
        assert.equal(back, lines(v.settings).map((l) => (l.startsWith('  "s": ') ? sLine : l)).join('\n'));
      }
    });
  }
});

describe('untrusted keys never reach a prototype', () => {
  test('an event key "__proto__" under hooks survives install and uninstall as an ordinary key', () => {
    // The settings file is shared with the Rust port (installer-vectors/a-proto-event-key).
    const v = readInstallerVectors().cases.find((c) => c.group === 'proto');
    const text = v.settings;
    assert.ok(v.roundTrip && v.uninstalled === text);
    const dir = tempDir();
    const file = path.join(dir, 'settings.json');
    fs.writeFileSync(file, text);
    const data = path.join(dir, 'data');
    assert.equal(cli(['hooks', 'install', '--write', '--settings', file], data).code, 0);
    const installed = JSON.parse(fs.readFileSync(file, 'utf8'));
    assert.ok(Object.hasOwn(installed.hooks, '__proto__'));
    assert.equal(Object.getPrototypeOf(installed.hooks), Object.prototype);
    assert.equal(cli(['hooks', 'uninstall', '--write', '--settings', file], data).code, 0);
    assert.equal(fs.readFileSync(file, 'utf8'), text);
  });

  for (const flag of [['--__proto__=x'], ['--__proto__', 'x'], ['--constructor', 'x'], ['--toString']]) {
    test(`hooks install --write ${flag.join(' ')}: exit 3`, () => {
      const r = cli(['hooks', 'install', '--write', ...flag], path.join(tempDir(), 'data'));
      assert.equal(r.code, 3, r.stderr);
      assert.match(r.stderr, /unknown option/);
    });
  }
});

describe('the install record is advisory', () => {
  for (const content of ['null', '"x"', '5', '[]', '{"/x": null, "/y": {"events": "all", "hooks": 1}}', '{bad']) {
    test(`installs.json = ${content}: install warns, succeeds, reports the backup; uninstall restores`, () => {
      const { dir, file } = copyFixture();
      const data = path.join(dir, 'data');
      fs.mkdirSync(data, { recursive: true });
      fs.writeFileSync(path.join(data, 'installs.json'), content);
      const r = cli(['hooks', 'install', '--write', '--settings', file], data);
      assert.equal(r.code, 0, r.stderr);
      assert.match(r.stderr, /warning: .*installs\.json/);
      assert.match(r.stderr, /backup of the previous file: /);
      assert.doesNotMatch(r.stderr, /TypeError|at .*\.mjs:\d+/);
      const record = JSON.parse(fs.readFileSync(path.join(data, 'installs.json'), 'utf8'));
      assert.equal(Object.keys(record).length, 1, 'the rewritten record holds only valid entries');
      assert.equal(cli(['hooks', 'uninstall', '--write', '--settings', file], data).code, 0);
      assert.ok(deepEqual(JSON.parse(fs.readFileSync(file, 'utf8')), fixture()));
    });
  }
});

describe('--out never destroys data', () => {
  const dataDir = () => path.join(tempDir(), 'data');

  test('hooks print --out may replace its own earlier output, and refuses anything else without --force', () => {
    const dir = tempDir();
    const out = path.join(dir, 'hooks-only.json');
    assert.equal(cli(['hooks', 'print', '--out', out], dataDir()).code, 0);
    assert.equal(cli(['hooks', 'print', '--out', out], dataDir()).code, 0, 'a bellwether hooks file can be regenerated');
    const notes = path.join(dir, 'notes.json');
    fs.writeFileSync(notes, '{"mine": true}');
    const r = cli(['hooks', 'print', '--out', notes], dataDir());
    assert.equal(r.code, 3);
    assert.match(r.stderr, /not a bellwether hooks file/);
    assert.equal(fs.readFileSync(notes, 'utf8'), '{"mine": true}');
    assert.equal(cli(['hooks', 'print', '--out', notes, '--force'], dataDir()).code, 0);
    assert.deepEqual(JSON.parse(fs.readFileSync(notes, 'utf8')), hookBlock(PORT, TOKEN));
  });

  for (const name of ['settings.json', 'settings.local.json']) {
    test(`hooks print --out .claude/${name} is refused even with --force`, () => {
      const dir = path.join(tempDir(), '.claude');
      fs.mkdirSync(dir);
      const f = path.join(dir, name);
      const r = cli(['hooks', 'print', '--out', f, '--force'], dataDir());
      assert.equal(r.code, 3);
      assert.match(r.stderr, /Claude Code settings file/);
      assert.equal(fs.existsSync(f), false);
    });
  }

  test('hooks print --out through a symlink to a settings file is refused', () => {
    const dir = tempDir();
    fs.mkdirSync(path.join(dir, '.claude'));
    const real = path.join(dir, '.claude', 'settings.json');
    fs.writeFileSync(real, '{}');
    const link = path.join(dir, 'innocent.json');
    fs.symlinkSync(real, link);
    assert.equal(cli(['hooks', 'print', '--out', link, '--force'], dataDir()).code, 3);
    assert.equal(fs.readFileSync(real, 'utf8'), '{}');
  });

  test('--force is refused on install and uninstall', () => {
    const { file } = copyFixture();
    assert.equal(cli(['hooks', 'install', '--force', '--settings', file], dataDir()).code, 3);
  });
});

describe('exact-URL ownership (Decision 13)', () => {
  const foreign = [
    { type: 'http', url: `http://127.0.0.1:${PORT}/hooks` },
    { type: 'http', url: `http://127.0.0.1:${PORT}/hook2` },
    { type: 'http', url: `http://127.0.0.1:${PORT}/hook?x=1` },
    { type: 'http', url: `http://127.0.0.1:${PORT}/hook/sub` },
  ];

  test('only the exact URL is bellwether\'s', () => {
    assert.ok(isBellwetherHandler({ type: 'http', url: `http://127.0.0.1:${PORT}/hook` }, PORT));
    for (const h of foreign) assert.ok(!isBellwetherHandler(h, PORT), h.url);
  });

  test('foreign …/hooks, …/hook2, …/hook?x and …/hook/sub survive install and uninstall byte-identical', () => {
    const text = JSON.stringify({ hooks: { Stop: [{ hooks: foreign }], PreToolUse: [{ matcher: 'Bash', hooks: [foreign[0]] }] } }, null, 2) + '\n';
    const dir = tempDir();
    const file = path.join(dir, 'settings.json');
    fs.writeFileSync(file, text);
    const data = path.join(dir, 'data');
    assert.equal(cli(['hooks', 'install', '--write', '--settings', file], data).code, 0);
    const installed = JSON.parse(fs.readFileSync(file, 'utf8'));
    assert.deepEqual(installed.hooks.Stop[0].hooks, foreign, 'foreign handlers untouched by install');
    assert.equal(cli(['hooks', 'uninstall', '--write', '--settings', file], data).code, 0);
    assert.equal(fs.readFileSync(file, 'utf8'), text);
  });

  test('doctor neither counts nor flags a foreign …/hooks handler', async () => {
    const checks = checkSettings({ hooks: { Stop: [{ hooks: foreign }] } }, { port: PORT, token: TOKEN });
    assert.equal(checks.find((c) => c.name === 'hooks installed').found, 0);
    assert.equal(checks.find((c) => c.name === 'hooks stray'), undefined);
  });

  test('after a port change, old-port entries are flagged by doctor and left alone until `uninstall --port <old>`', async () => {
    const dir = tempDir();
    const file = path.join(dir, 'settings.json');
    const oldPort = 47500;
    const settings = install(fixture(), oldPort, TOKEN);
    fs.writeFileSync(file, JSON.stringify(settings, null, 2) + '\n');
    const stray = checkSettings(settings, { port: PORT, token: TOKEN }).find((c) => c.name === 'hooks stray');
    assert.equal(stray.status, 'WARN');
    assert.match(stray.detail, /bellwether hooks uninstall --port 47500 --write/);
    const data = path.join(dir, 'data');
    // install on the configured port adds its own groups and leaves the old ones
    assert.equal(cli(['hooks', 'install', '--write', '--settings', file], data).code, 0);
    const both = JSON.parse(fs.readFileSync(file, 'utf8'));
    assert.equal(Object.keys(countGroups(both, oldPort)).length, 16);
    assert.equal(Object.keys(countGroups(both, PORT)).length, 16);
    // a plain uninstall removes only the configured port's entries
    assert.equal(cli(['hooks', 'uninstall', '--write', '--settings', file], data).code, 0);
    assert.equal(Object.keys(countGroups(JSON.parse(fs.readFileSync(file, 'utf8')), oldPort)).length, 16);
    // naming the old port removes the leftovers and restores the original
    assert.equal(cli(['hooks', 'uninstall', '--port', String(oldPort), '--write', '--settings', file], data).code, 0);
    assert.ok(deepEqual(JSON.parse(fs.readFileSync(file, 'utf8')), fixture()));
  });

  test('--port is refused on install and print, and must be a port number', () => {
    const { file } = copyFixture();
    const data = path.join(tempDir(), 'data');
    assert.equal(cli(['hooks', 'install', '--port', '47500', '--settings', file], data).code, 3);
    assert.equal(cli(['hooks', 'print', '--port', '47500'], data).code, 3);
    assert.equal(cli(['hooks', 'uninstall', '--port', '47abc', '--settings', file], data).code, 3);
  });
});
