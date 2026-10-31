import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { AGENT_DIR } from './helpers.mjs';
import { PETS, TEST_PORT, cliCmd, readme, runTestCmd, testConfig } from '../scripts/build-windows-test.mjs';

/**
 * Windows plan W1: the test folder the maintainer runs on his PC (scripts/build-windows-test.mjs)
 * and the Windows CI job. The folder never touches the live install: a scratch home, a test
 * port, the demo only. The workflow is read as text, as release.test.mjs reads it.
 */

const read = (...p) => fs.readFileSync(path.join(AGENT_DIR, ...p), 'utf8');

/** One job's text from a workflow (two-space-indented name under `jobs:`). */
function job(text, name) {
  const body = text.slice(text.indexOf('\njobs:\n') + 7);
  const start = body.indexOf(`\n  ${name}:\n`);
  assert.ok(start >= 0, `no job ${name}`);
  const rest = body.slice(start + 1);
  const next = rest.slice(1).search(/\n {2}[A-Za-z0-9_-]+:\n/);
  return next < 0 ? rest : rest.slice(0, next + 1);
}

describe('the Windows test folder (W1, WD12)', () => {
  test('its port is never the live 47417, and config.json is the example on that port', () => {
    assert.notEqual(TEST_PORT, 47417);
    const example = read('config.example.json');
    const cfg = testConfig(example);
    assert.equal(JSON.parse(cfg).port, TEST_PORT);
    const changed = cfg.split('\n').filter((l, i) => l !== example.split('\n')[i]);
    assert.deepEqual(changed, [`  "port": ${TEST_PORT},`], 'only the port line differs');
    assert.throws(() => testConfig('{"nested": {"port": 1}}'), /top-level "port"/);
  });

  test('run-test.cmd: CRLF, a scratch home beside it, and only the demo, the sessions or the dump', () => {
    const cmd = runTestCmd();
    assert.ok(cmd.endsWith('\r\n') && !/[^\r]\n/.test(cmd), 'CRLF line endings throughout');
    const lines = cmd.split('\r\n');
    assert.ok(lines.includes('set "BELLWETHER_HOME=%HERE%home"'));
    assert.ok(lines.includes('set "BELLWETHER_AGENT_DIR=%HERE:~0,-1%"'));
    for (const v of ['BELLWETHER_CONFIG', 'BELLWETHER_DATA_DIR', 'BELLWETHER_HOOK_TOKEN']) assert.ok(lines.includes(`set "${v}="`), `${v} cleared`);
    const runs = lines.filter((l) => l.includes('Bellwether.exe"'));
    assert.equal(runs.length, 3);
    // `--demo` comes first, so whatever is passed after it is the app's flags, never a CLI
    // subcommand (dispatch.rs: a first argument starting with `-` is the app's).
    assert.match(runs[0], /^start "" \/wait "%HERE%Bellwether\.exe" --demo --demo-monitors --heartbeat %\* --quit-after 1800$/);
    // W2: real sessions, the strip with no argument at all (it starts its own brain).
    assert.match(runs[1], /^start "" \/wait "%HERE%Bellwether\.exe"$/);
    assert.match(runs[2], /^start "" \/wait "%HERE%Bellwether\.exe" --dump-desktop "%~f2"$/);
    // --sessions: the token made once, and the hooks as a file for claude --settings,
    // through cli.cmd (the folder's home and config), stopping on a failure.
    const sessions = lines.slice(lines.indexOf(':sessions'), lines.indexOf(':dump'));
    const calls = sessions.filter((l) => l.startsWith('call "'));
    assert.deepEqual(calls, ['call "%HERE%cli.cmd" token init', 'call "%HERE%cli.cmd" hooks print --out "%HERE%hooks.json"']);
    assert.equal(sessions.filter((l) => l === 'if errorlevel 1 exit /b 1').length, 2);
    assert.ok(sessions.some((l) => l.includes('claude --settings "%HERE%hooks.json"')));
    // A failed dump says why (shell.log's --dump-desktop lines) and exits non-zero.
    const dump = lines.slice(lines.indexOf(':dump'));
    assert.ok(dump.includes('set "RC=%ERRORLEVEL%"') && dump.includes('exit /b %RC%') && dump.some((l) => l.startsWith('findstr /c:"--dump-desktop:"')));
    // Each run starts a fresh log; the three before are kept, oldest moved first.
    const demo = lines.slice(0, lines.indexOf(':rotate'));
    assert.ok(demo.indexOf('call :rotate') >= 0 && demo.indexOf('call :rotate') < demo.findIndex((l) => l.includes('--demo --demo-monitors')));
    assert.ok(sessions.indexOf('call :rotate') >= 0 && sessions.indexOf('call :rotate') < sessions.findIndex((l) => l.startsWith('start ')));
    const rotate = lines.slice(lines.indexOf(':rotate'), lines.indexOf(':sessions'));
    const moves = rotate.filter((l) => l.includes(' move /y ')).map((l) => l.match(/shell[\w.-]*\.log/g));
    assert.deepEqual(moves, [
      ['shell.previous-2.log', 'shell.previous-2.log', 'shell.previous-3.log'],
      ['shell.previous.log', 'shell.previous.log', 'shell.previous-2.log'],
      ['shell.log', 'shell.log', 'shell.previous.log'],
    ]);
    assert.equal(rotate.at(-1), 'goto :eof');
    const code = lines.filter((l) => !l.startsWith('rem') && !l.startsWith('echo')).join('\n');
    assert.ok(!/APPDATA|USERPROFILE|\.claude|47417|autostart|install/.test(code), 'nothing of the live install');
    assert.deepEqual(code.match(/hooks [a-z]+/g), ['hooks print'], 'hooks are printed to a file, never installed');
  });

  test('cli.cmd: CRLF, the console CLI with the folder\'s home and config (W2)', () => {
    const cmd = cliCmd();
    assert.ok(cmd.endsWith('\r\n') && !/[^\r]\n/.test(cmd), 'CRLF line endings throughout');
    const lines = cmd.split('\r\n');
    for (const l of ['setlocal', 'set "BELLWETHER_HOME=%HERE%home"', 'set "BELLWETHER_CONFIG=%HERE%config.json"', 'set "BELLWETHER_AGENT_DIR=%HERE:~0,-1%"', 'set "BELLWETHER_DATA_DIR="', 'set "BELLWETHER_HOOK_TOKEN="', '"%HERE%bin\\bellwether.exe" %*', 'exit /b %ERRORLEVEL%']) {
      assert.ok(lines.includes(l), l);
    }
    assert.ok(!/APPDATA|USERPROFILE|\.claude|47417/.test(lines.filter((l) => !l.startsWith('rem')).join('\n')));
  });

  test('the README says what it never touches and how to clean up', () => {
    const r = readme();
    assert.match(r, /never reads\r\nor writes %APPDATA%\\Bellwether or %USERPROFILE%\\\.claude, never uses port 47417/);
    assert.ok(r.includes(`%LOCALAPPDATA%\\local.bellwether.p${TEST_PORT}`), 'WebView2 folder named after the per-port identifier (lib.rs single_instance_id)');
  });

  test('it carries Bellwether\'s own pets, the ones the brain bundles', () => {
    const pets = read('crates', 'brain', 'src', 'pets.rs');
    const bundled = /pub const BUNDLED: \[&str; \d+\] = \[([^\]]*)\]/.exec(pets)[1].match(/"([^"]+)"/g).map((s) => s.slice(1, -1));
    assert.deepEqual(PETS, bundled);
  });

  test('the test-build feature is only ever turned on by this script', () => {
    const cargo = read('shell', 'src-tauri', 'Cargo.toml');
    assert.match(cargo, /^test-build = \[\]$/m);
    for (const f of ['scripts/build-app.mjs', 'scripts/release.mjs', '.github/workflows/release.yml', 'shell/src-tauri/tauri.conf.json', 'shell/src-tauri/tauri.dist.conf.json', 'package.json']) {
      assert.ok(!read(f).includes('test-build'), f);
    }
    assert.ok(read('scripts', 'build-windows-test.mjs').includes("'--features', 'test-build,console-cli'"));
    // The console CLI (W2, WD7) is only built with its feature, so macOS builds are unchanged.
    assert.match(cargo, /\[\[bin\]\]\nname = "bellwether"\npath = "src\/bin\/bellwether\.rs"\nrequired-features = \["console-cli"\]/);
    assert.match(cargo, /^default-run = "bellwether-shell"$/m);
  });
});

describe('the Windows CI job (W1)', () => {
  const ci = read('.github', 'workflows', 'ci.yml');
  const win = job(ci, 'windows');

  test('runs on the pinned windows-2025 image, with no secret and no environment', () => {
    assert.match(win, /^ {4}runs-on: windows-2025$/m);
    assert.ok(!/secrets\.|environment:/.test(win));
  });

  test('checks the whole workspace and runs npm run test:cargo, LF checkout first', () => {
    const steps = win.split('\n');
    const at = (re) => steps.findIndex((l) => re.test(l));
    assert.ok(at(/core\.autocrlf false/) >= 0 && at(/core\.autocrlf false/) < at(/actions\/checkout@/), 'LF line endings before the checkout');
    assert.ok(at(/cargo check --locked --manifest-path shell\/src-tauri\/Cargo\.toml --workspace --all-targets/) >= 0);
    assert.ok(at(/run: npm run test:cargo/) >= 0);
    assert.ok(at(/node scripts\/build-windows-test\.mjs$/) >= 0);
    // W2: conformance, the black-box hook suite, the console CLI's exit codes and refusals.
    assert.ok(at(/run: npm run test:cargo -- --no-fail-fast --features bellwether-shell\/conformance$/) >= 0);
    assert.ok(at(/node --import \.\/test\/setup\.mjs --test .*test\/hook-route-blackbox\.test\.mjs$/) >= 0);
    assert.match(win, /cli\.cmd hooks install --write 2>&1"\n\s+if \(\$LASTEXITCODE -ne 3 /);
    assert.match(win, /name: bellwether-windows-test\n/);
  });

  test('the smoke run says when it is skipped, never passes silently', () => {
    assert.match(win, /::warning title=Windows smoke run skipped::/);
    assert.match(win, /--quit-after reached/);
    assert.match(win, /timeout-minutes: 3/);
  });
});

describe('Windows exe names (W1-11)', () => {
  // Windows' installer detection asks for elevation (os error 740) before running an exe
  // whose name contains one of these words, and every cargo test target is an exe there.
  const banned = /install|setup|update|patch/i;
  test('no cargo test target, binary or package is named with install, setup, update or patch', () => {
    const names = [];
    for (const dir of ['crates/core/tests', 'crates/brain/tests', 'shell/src-tauri/tests']) {
      for (const f of fs.readdirSync(path.join(AGENT_DIR, dir))) if (f.endsWith('.rs')) names.push(`${dir}/${f}`.replace(/\.rs$/, '').split('/').pop());
    }
    assert.ok(names.includes('hook_vectors'), names.join(', '));
    for (const toml of ['crates/core/Cargo.toml', 'crates/brain/Cargo.toml', 'shell/src-tauri/Cargo.toml']) {
      const t = read(...toml.split('/'));
      for (const m of t.matchAll(/^name = "([^"]+)"/gm)) names.push(m[1]);
    }
    const bad = names.filter((n) => banned.test(n));
    assert.deepEqual(bad, [], 'rename these (Windows plan, deviation W1-11)');
  });
});
