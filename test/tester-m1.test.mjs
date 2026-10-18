import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import { after, before, describe, test } from 'node:test';
import { AGENT_DIR, FIXTURES, TOKEN, hookHeaders, spawnBrain, spawnBrainSync, tempDir } from './helpers.mjs';

/**
 * Independent tester probes for Milestone 1. Everything runs in a temp directory (its own
 * config and port, own data dir, HOME pointed at a temp dir), so nothing here can reach
 * ~/.claude/settings.json or the real .env.
 */

const freePort = () =>
  new Promise((resolve) => {
    const s = net.createServer().listen(0, '127.0.0.1', () => {
      const { port } = s.address();
      s.close(() => resolve(port));
    });
  });

let root, home, port, env, settingsPath;

function cli(args, { input, extraEnv = {}, cwd } = {}) {
  const r = spawnBrainSync(args, {
    root,
    encoding: 'utf8',
    env: { ...env, ...extraEnv },
    input,
    cwd: cwd ?? root,
    timeout: 20000,
  });
  return { code: r.status, out: r.stdout, err: r.stderr };
}

// Async twin of cli(): required whenever the test itself is serving on a port, because
// spawnSync would block this process's event loop and the in-process server could not answer.
function cliAsync(args) {
  return new Promise((resolve) => {
    const c = spawnBrain([...args], { root, env, cwd: root });
    let out = '', err = '';
    c.stdout.on('data', (d) => (out += d));
    c.stderr.on('data', (d) => (err += d));
    c.on('close', (code) => resolve({ code, out, err }));
  });
}

function raw(p, text, { wait = 300 } = {}) {
  return new Promise((resolve) => {
    const chunks = [];
    const sock = net.connect(p, '127.0.0.1', () => sock.write(text));
    sock.on('data', (c) => chunks.push(c));
    const done = () => {
      sock.destroy();
      resolve(Buffer.concat(chunks).toString('latin1'));
    };
    sock.on('close', done);
    sock.on('error', done);
    setTimeout(done, wait);
  });
}
const split = (resp) => {
  const i = resp.indexOf('\r\n\r\n');
  return i === -1 ? { head: resp, body: '' } : { head: resp.slice(0, i), body: resp.slice(i + 4) };
};

before(async () => {
  root = tempDir('bellwether-br3k-');
  home = path.join(root, 'home');
  fs.mkdirSync(path.join(home, '.claude'), { recursive: true });
  port = await freePort();
  const cfg = JSON.parse(fs.readFileSync(path.join(AGENT_DIR, 'config.example.json'), 'utf8'));
  cfg.port = port;
  fs.writeFileSync(path.join(root, 'config.example.json'), JSON.stringify(cfg));
  settingsPath = path.join(home, '.claude', 'settings.json');
  env = { PATH: process.env.PATH, HOME: home, USERPROFILE: home, BELLWETHER_DATA_DIR: path.join(root, 'data'), BELLWETHER_HOOK_TOKEN: TOKEN };
});
after(() => fs.rmSync(root, { recursive: true, force: true }));

function startBrain(extraArgs = [], extraEnv = {}) {
  const child = spawnBrain(['serve', ...extraArgs], { root, env: { ...env, ...extraEnv }, stdio: ['ignore', 'pipe', 'pipe'] });
  let err = '';
  child.stderr.on('data', (d) => (err += d));
  return new Promise((resolve, reject) => {
    const t = setInterval(() => {
      if (err.includes('listening on')) {
        clearInterval(t);
        resolve({ child, stderr: () => err, stop: () => new Promise((r) => (child.once('exit', r), child.kill('SIGTERM'))) });
      }
      if (child.exitCode !== null) {
        clearInterval(t);
        reject(new Error('brain exited: ' + err));
      }
    }, 20);
  });
}

describe('hook endpoint: every path and method answers without a body', () => {
  let brain;
  before(async () => {
    brain = await startBrain();
  });
  after(() => brain.stop());

  const methods = ['GET', 'HEAD', 'PUT', 'DELETE', 'PATCH', 'OPTIONS', 'TRACE', 'POST'];
  const paths = ['/hook', '/hook/', '/hook?x=1', '/HOOK', '//hook', '/hook%2F', '/hook%00', '/hooks', '/state', '/control', '/', '/health/x', '/health', '*', 'http://127.0.0.1/hook'];
  test('no response to anything but GET /health carries a body or a Content-Type', async () => {
    const offenders = [];
    for (const m of methods)
      for (const p of paths) {
        const good = `Host: 127.0.0.1:${port}\r\nContent-Type: application/json\r\nX-Bellwether-Token: ${TOKEN}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}`;
        const bad = `Host: 127.0.0.1:${port}\r\nConnection: close\r\n\r\n`;
        for (const hdrs of [good, bad]) {
          const { head, body } = split(await raw(port, `${m} ${p} HTTP/1.1\r\n${hdrs}`));
          const isHealth = m === 'GET' && p === '/health';
          if (isHealth) continue;
          // M2 (plan: Brain HTTP API): /state and /control answer JSON, but only to a request
          // that carries the token. Unauthenticated, they stay empty like everything else.
          if (hdrs === good && (p === '/state' || p === '/control')) {
            if (/access-control/i.test(head)) offenders.push(`${m} ${p}: CORS header on an authenticated answer`);
            if (body.length > 0 && !/content-type: application\/json/i.test(head)) offenders.push(`${m} ${p}: authenticated body that is not JSON`);
            continue;
          }
          if (m === 'HEAD' && /content-length: [1-9]/i.test(head)) offenders.push(`${m} ${p}: HEAD advertises length`);
          if (body.length > 0) offenders.push(`${m} ${p}: body ${JSON.stringify(body.slice(0, 40))}`);
          if (/content-type/i.test(head)) offenders.push(`${m} ${p}: content-type`);
          if (/access-control/i.test(head)) offenders.push(`${m} ${p}: CORS header`);
        }
      }
    assert.deepEqual(offenders, []);
  });

  test('malformed requests and smuggling attempts get no body either', async () => {
    const cases = [
      'GARBAGE\r\n\r\n',
      `POST /hook HTTP/1.1\r\nHost: 127.0.0.1:${port}\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\n`,
      `POST /hook HTTP/1.1\r\nHost: 127.0.0.1:${port}\r\nContent-Length: 4\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n`,
      `POST /hook HTTP/1.0\r\nContent-Type: application/json\r\nX-Bellwether-Token: ${TOKEN}\r\nContent-Length: 2\r\n\r\n{}`,
      `POST /hook HTTP/1.1\r\nHost: 127.0.0.1:${port}\r\nHost: evil.example\r\nContent-Type: application/json\r\nX-Bellwether-Token: ${TOKEN}\r\nContent-Length: 2\r\n\r\n{}`,
      `POST /hook HTTP/1.1\r\nHost: 127.0.0.1:${port}\r\nContent-Type: application/json\r\nX-Bellwether-Token: ${TOKEN}\r\nContent-Length: 2\r\nExpect: 100-continue\r\n\r\n{}`,
    ];
    for (const c of cases) {
      const resp = (await raw(port, c)).replace(/^HTTP\/1\.1 100 Continue\r\n\r\n/, '');
      const { body } = split(resp);
      assert.equal(body, '', `body for ${JSON.stringify(c.slice(0, 60))}`);
    }
  });

  test('chunked oversize body (9 MiB, no Content-Length) still ends in an empty 204', async () => {
    const chunk = Buffer.alloc(1024 * 1024, 0x20);
    const resp = await new Promise((resolve) => {
      const req = http.request({ host: '127.0.0.1', port, method: 'POST', path: '/hook', headers: { Host: `127.0.0.1:${port}`, 'Content-Type': 'application/json', 'X-Bellwether-Token': TOKEN, 'Transfer-Encoding': 'chunked' } }, (res) => {
        const c = [];
        res.on('data', (d) => c.push(d));
        res.on('end', () => resolve({ status: res.statusCode, len: Buffer.concat(c).length }));
      });
      for (let i = 0; i < 9; i++) req.write(chunk);
      req.end();
    });
    assert.deepEqual(resp, { status: 204, len: 0 });
  });

  test('a good request with a hostile hook_event_name or deeply nested tool_input is still an empty 204, and the brain survives', async () => {
    const deep = '['.repeat(200000) + ']'.repeat(200000);
    for (const body of [JSON.stringify({ hook_event_name: { a: 1 }, session_id: [] }), `{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":${deep}}`, '{"__proto__":{"polluted":true},"hook_event_name":"Stop"}']) {
      const { head, body: b } = split(await raw(port, `POST /hook HTTP/1.1\r\nHost: 127.0.0.1:${port}\r\nContent-Type: application/json\r\nX-Bellwether-Token: ${TOKEN}\r\nContent-Length: ${Buffer.byteLength(body)}\r\nConnection: close\r\n\r\n${body}`, { wait: 800 }));
      assert.match(head, /^HTTP\/1.1 204/);
      assert.equal(b, '');
    }
    const h = split(await raw(port, `GET /health HTTP/1.1\r\nHost: 127.0.0.1:${port}\r\nConnection: close\r\n\r\n`));
    assert.match(h.head, /200/);
    assert.equal(({}).polluted, undefined);
  });

  test('DNS-rebinding style Hosts are refused even with a valid token', async () => {
    for (const host of [`evil.example:${port}`, `127.0.0.1.evil.example:${port}`, `localhost.evil.example:${port}`, '127.0.0.1', `localhost`, `0.0.0.0:${port}`, `[::1]:${port}`, `127.1:${port}`, `2130706433:${port}`]) {
      const resp = await raw(port, `POST /hook HTTP/1.1\r\nHost: ${host}\r\nContent-Type: application/json\r\nX-Bellwether-Token: ${TOKEN}\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}`);
      assert.match(resp, /^HTTP\/1.1 403/, `Host: ${host}`);
    }
  });

  test('the listener is bound to 127.0.0.1 only', async () => {
    const lsof = spawnSync('lsof', ['-nP', `-iTCP:${port}`, '-sTCP:LISTEN'], { encoding: 'utf8' });
    if (lsof.error) return; // no lsof on this OS; the connection checks below still apply
    assert.match(lsof.stdout, new RegExp(`127\\.0\\.0\\.1:${port}`));
    assert.doesNotMatch(lsof.stdout, /\*:\d+|\[::\]|0\.0\.0\.0/);
    const nonLoopback = Object.values(os.networkInterfaces()).flat().find((i) => i && !i.internal && i.family === 'IPv4');
    if (nonLoopback) {
      const ok = await new Promise((r) => {
        const s = net.connect(port, nonLoopback.address);
        s.on('connect', () => (s.destroy(), r(true)));
        s.on('error', () => r(false));
      });
      assert.equal(ok, false, `reachable on ${nonLoopback.address}`);
    }
  });
});

describe('recorder keeps no free text', () => {
  test('canary strings in every content-bearing field never reach the recording or the log', async () => {
    const rec = path.join(root, 'canary.jsonl');
    const child = spawnBrain(['record', '--out', rec], { root, env, stdio: ['ignore', 'pipe', 'pipe'] });
    let err = '';
    child.stderr.on('data', (d) => (err += d));
    await new Promise((r) => { const t = setInterval(() => err.includes('recording to') && (clearInterval(t), r()), 20); });
    const C = (n) => `CANARY_${n}_xyz`;
    const events = [
      { hook_event_name: 'UserPromptSubmit', session_id: 's1', cwd: '/x/proj', prompt: C('prompt'), transcript_path: `/t/${C('transcript')}.jsonl`, scratchpad_dir: `/s/${C('scratch')}` },
      { hook_event_name: 'PostToolUse', session_id: 's1', tool_name: 'Bash', tool_use_id: 'tu1', tool_input: { command: `echo ${C('cmd')}`, description: C('desc') }, tool_response: { stdout: C('response'), stderr: C('stderr') } },
      { hook_event_name: 'PostToolUseFailure', session_id: 's1', tool_name: 'Bash', tool_use_id: 'tu1', error: `failed ${C('error')}`, tool_input: { command: C('cmd2') } },
      { hook_event_name: 'PostCompact', session_id: 's1', trigger: 'manual', compact_summary: C('summary') },
      { hook_event_name: 'PreCompact', session_id: 's1', trigger: 'auto', custom_instructions: C('custom') },
      { hook_event_name: 'Stop', session_id: 's1', last_assistant_message: C('assistant'), background_tasks: [{ name: C('bg') }] },
      { hook_event_name: 'SubagentStop', session_id: 's1', agent_id: 'a1', agent_type: 'Explore', last_assistant_message: C('sub'), agent_transcript_path: `/t/${C('agtr')}` },
      { hook_event_name: 'Notification', session_id: 's1', notification_type: 'permission_prompt', message: `Claude needs ${C('msg')}` },
      { hook_event_name: 'PermissionRequest', session_id: 's1', tool_name: 'Write', tool_input: { file_path: `/a/${C('path')}`, content: C('content') }, permission_suggestions: [{ rules: [{ toolName: C('rule') }] }] },
      { hook_event_name: 'Elicitation', session_id: 's1', mcp_server_name: 'srv', message: C('elic'), mode: 'form', requested_schema: { properties: { [C('schemakey')]: {} } } },
      { hook_event_name: 'ElicitationResult', session_id: 's1', mcp_server_name: 'srv', form_values: { a: C('form') } },
      { hook_event_name: 'StopFailure', session_id: 's1', error: 'rate_limit', error_details: C('details') },
      { hook_event_name: 'SessionEnd', session_id: 's1', reason: 'other' },
    ];
    for (const e of events) {
      const body = JSON.stringify(e);
      const res = await new Promise((resolve) => {
        const q = http.request({ host: '127.0.0.1', port, method: 'POST', path: '/hook', headers: hookHeaders(port, { 'Content-Length': Buffer.byteLength(body), 'X-Bellwether-Herdr-Pane': 'w1:p1' }) }, (r) => (r.resume(), r.on('end', () => resolve(r.statusCode))));
        q.end(body);
      });
      assert.equal(res, 204);
    }
    await new Promise((r) => setTimeout(r, 300));
    child.kill('SIGTERM');
    await new Promise((r) => child.once('exit', r));
    const recorded = fs.readFileSync(rec, 'utf8');
    assert.equal(recorded.trim().split('\n').length, events.length);
    for (const blob of [recorded, err]) assert.doesNotMatch(blob, /CANARY_/);
    // the allowlisted identity fields do survive, so the check above is not vacuous
    assert.match(recorded, /"toolFingerprint"/);
    assert.match(recorded, /"herdrPane":"w1:p1"/);
  });
});

describe('installer ownership and unusual settings shapes', () => {
  const run = (name, obj, args = ['hooks', 'install', '--write'], { text } = {}) => {
    const f = path.join(root, `${name}.json`);
    fs.writeFileSync(f, text ?? JSON.stringify(obj, null, 2) + '\n');
    return { f, ...cli([...args, '--settings', f]) };
  };
  const hookCmd = (c) => ({ hooks: [{ type: 'command', command: c }] });
  const sh = (f) => fs.readFileSync(f, 'utf8');

  test('group-level garbage ({hooks: "string"}, {hooks: {}}) does not crash install or doctor with a stack trace', () => {
    for (const bad of ['text', {}, 5, null]) {
      const r = run(`weird-${typeof bad}`, { hooks: { Stop: [{ hooks: bad }] } });
      assert.doesNotMatch(r.err, /TypeError|at .*\.mjs:\d+/, `install with hooks=${JSON.stringify(bad)} gave a stack trace: ${r.err.slice(0, 200)}`);
      const d = cli(['doctor', '--settings', r.f]);
      assert.doesNotMatch(d.err, /TypeError|at .*\.mjs:\d+/, `doctor with hooks=${JSON.stringify(bad)} gave a stack trace: ${d.err.slice(0, 200)}`);
    }
  });

  test('install then uninstall restores events that were present but empty', () => {
    const original = { hooks: { Stop: [], PreToolUse: [hookCmd('keep')] } };
    const r = run('empty-event', original);
    assert.equal(r.code, 0);
    cli(['hooks', 'uninstall', '--write', '--settings', r.f]);
    assert.deepEqual(JSON.parse(sh(r.f)), original);
  });

  test('an empty "hooks": {} object does not block install', () => {
    const r = run('empty-hooks', { theme: 'dark', hooks: {} });
    assert.equal(r.code, 0, r.err);
  });

  test('numbers beyond double precision survive an install/uninstall round trip', () => {
    const text = '{\n  "bigNumber": 12345678901234567890,\n  "hooks": {\n    "Stop": [{ "hooks": [{ "type": "command", "command": "k" }] }]\n  }\n}\n';
    const r = run('bigint', null, ['hooks', 'install', '--write'], { text });
    assert.equal(r.code, 0, r.err);
    cli(['hooks', 'uninstall', '--write', '--settings', r.f]);
    assert.match(sh(r.f), /12345678901234567890/);
  });

  test('a CRLF settings file keeps every existing line in the dry-run diff', () => {
    const text = JSON.stringify({ hooks: { Stop: [hookCmd('keep')] }, theme: 'dark' }, null, 2).replace(/\n/g, '\r\n') + '\r\n';
    const r = run('crlf', null, ['hooks', 'install'], { text });
    const removed = r.out.split('\n').filter((l) => l.startsWith('-') && !l.startsWith('---'));
    assert.equal(removed.length, 0, `dry run shows ${removed.length} existing lines removed`);
  });

  test('install then uninstall of a 4-space indented file is byte-identical', () => {
    const text = JSON.stringify({ theme: 'dark', hooks: { Stop: [hookCmd('keep')] } }, null, 4) + '\n';
    const r = run('indent4', null, ['hooks', 'install', '--write'], { text });
    cli(['hooks', 'uninstall', '--write', '--settings', r.f]);
    assert.equal(sh(r.f), text);
  });

  test('comments, trailing commas, a BOM and a top-level array are refused with exit 3 and nothing written', () => {
    const cases = { comment: '{\n // hi\n "hooks": {}\n}\n', trailing: '{"hooks": {},}', bom: '﻿{"hooks":{}}', array: '[]', empty: '' };
    const backups = () => (fs.existsSync(path.join(root, 'data', 'backups')) ? fs.readdirSync(path.join(root, 'data', 'backups')).length : 0);
    const n0 = backups();
    for (const [k, text] of Object.entries(cases)) {
      const r = run(`refuse-${k}`, null, ['hooks', 'install', '--write'], { text });
      assert.equal(r.code, 3, `${k}: exit ${r.code}, ${r.err.slice(0, 120)}`);
      assert.equal(sh(r.f), text);
      assert.doesNotMatch(r.err, /at .*\.mjs:\d+/);
    }
    assert.equal(backups(), n0, 'a refused file must not leave a backup');
  });

  test('other hooks on the same events: untouched, same order, bellwether group appended last', () => {
    const original = { hooks: { PreToolUse: [hookCmd('a'), { matcher: 'Bash', ...hookCmd('b') }, hookCmd('c')], Stop: [hookCmd('d')] }, statusLine: { type: 'command', command: 'x' } };
    const r = run('others', original);
    const after = JSON.parse(sh(r.f));
    assert.deepEqual(after.hooks.PreToolUse.slice(0, 3), original.hooks.PreToolUse);
    assert.equal(after.hooks.PreToolUse.length, 4);
    assert.deepEqual(after.statusLine, original.statusLine);
  });

  test('a foreign http hook on a different port or path is never claimed', () => {
    const foreign = { type: 'http', url: `http://127.0.0.1:${port + 1}/hook`, headers: { 'X-Other': '1' } };
    const foreign2 = { type: 'http', url: `http://127.0.0.1:${port}/other` };
    const foreign3 = { type: 'http', url: `http://localhost:${port}/hook` };
    const original = { hooks: { Stop: [{ hooks: [foreign, foreign2, foreign3] }] } };
    const r = run('foreign', original);
    cli(['hooks', 'uninstall', '--write', '--settings', r.f]);
    assert.deepEqual(JSON.parse(sh(r.f)), original);
  });

  test('an existing bellwether entry with an old token and a stray matcher is replaced, not duplicated', () => {
    const old = { type: 'http', url: `http://127.0.0.1:${port}/hook`, timeout: 600, headers: { 'X-Bellwether-Token': 'old' } };
    const r = run('old-entry', { hooks: { Stop: [{ matcher: 'x', hooks: [old] }, hookCmd('mine')] } });
    const s = JSON.parse(sh(r.f));
    const mine = s.hooks.Stop.flatMap((g) => g.hooks).filter((h) => h.url?.startsWith(`http://127.0.0.1:${port}/hook`));
    assert.equal(mine.length, 1);
    assert.equal(mine[0].timeout, 1);
    assert.deepEqual(s.hooks.Stop.find((g) => g.hooks[0].command === 'mine'), hookCmd('mine'));
  });

  test('a missing settings file: dry run writes nothing; --write creates it with no backup; uninstall leaves a valid file', () => {
    const f = path.join(home, 'newdir', 'settings.json');
    const dry = cli(['hooks', 'install', '--settings', f]);
    assert.equal(dry.code, 0);
    assert.equal(fs.existsSync(f), false);
    assert.equal(fs.existsSync(path.dirname(f)), false, 'dry run created a directory');
    assert.equal(cli(['hooks', 'install', '--write', '--settings', f]).code, 0);
    assert.equal(Object.keys(JSON.parse(sh(f)).hooks).length, 16);
    assert.equal(cli(['hooks', 'uninstall', '--write', '--settings', f]).code, 0);
    JSON.parse(sh(f));
  });

  test('a symlinked settings file stays a symlink and the target is updated', () => {
    const target = path.join(root, 'real-target.json');
    const link = path.join(root, 'link.json');
    fs.writeFileSync(target, '{\n  "theme": "dark"\n}\n');
    fs.symlinkSync(target, link);
    assert.equal(cli(['hooks', 'install', '--write', '--settings', link]).code, 0);
    assert.ok(fs.lstatSync(link).isSymbolicLink());
    assert.ok(JSON.parse(sh(target)).hooks.Stop);
  });

  test('file permissions are preserved on write, and the backup is the original bytes', () => {
    const f = path.join(root, 'perm.json');
    const text = '{\n  "theme": "dark"\n}\n';
    fs.writeFileSync(f, text, { mode: 0o600 });
    fs.chmodSync(f, 0o600);
    const r = cli(['hooks', 'install', '--write', '--settings', f]);
    assert.equal(fs.statSync(f).mode & 0o777, 0o600);
    const backup = r.err.match(/backup of the previous file: (.+)/)[1];
    assert.equal(fs.readFileSync(backup, 'utf8'), text);
  });

  test('--settings given without a value must not fall back to the real user settings file', () => {
    // HOME is a temp dir here, so "the real file" is <tmp>/.claude/settings.json.
    const original = '{\n  "theme": "dark"\n}\n';
    fs.writeFileSync(settingsPath, original);
    for (const args of [['hooks', 'install', '--write', '--settings'], ['hooks', 'install', '--settings', '--write'], ['hooks', 'install', '--write', '--settings=']]) {
      cli(args);
      assert.equal(sh(settingsPath), original, `${args.join(' ')} modified the default settings file`);
    }
  });

  test('hooks print -> hand-made install produce the same entries', () => {
    // The token is masked on stdout by design (M10-7); the real value goes only to --out files.
    const stdout = cli(['hooks', 'print']).out;
    assert.ok(!stdout.includes(TOKEN), 'hooks print wrote the real token to stdout');
    assert.match(stdout, new RegExp(`"X-Bellwether-Token": "${TOKEN.slice(0, 4)}…"`), 'stdout is masked to the first four characters');
    const outFile = path.join(root, 'print-out.json');
    assert.equal(cli(['hooks', 'print', '--out', outFile]).code, 0);
    const printed = JSON.parse(fs.readFileSync(outFile, 'utf8'));
    assert.equal(printed.hooks.Stop[0].hooks[0].headers['X-Bellwether-Token'], TOKEN);
    const r = run('print-vs-install', {});
    assert.deepEqual(JSON.parse(sh(r.f)).hooks, printed.hooks);
  });
});

describe('doctor', () => {
  const doctor = (file, extra = []) => cli(['doctor', '--json', '--settings', file, ...extra]);
  const settingsWith = (mutate) => {
    const f = path.join(root, `doc-${Math.random().toString(36).slice(2)}.json`);
    fs.writeFileSync(f, '{}\n');
    cli(['hooks', 'install', '--write', '--settings', f]);
    const s = JSON.parse(fs.readFileSync(f, 'utf8'));
    mutate(s);
    fs.writeFileSync(f, JSON.stringify(s, null, 2) + '\n');
    return f;
  };
  const check = (r, name) => JSON.parse(r.out).checks.find((c) => c.name === name);

  test('brain down: exit 2, "brain not running — run `bellwether serve`", and no token value in the output', () => {
    const f = settingsWith(() => {});
    const r = cli(['doctor', '--settings', f]);
    assert.equal(r.code, 2);
    assert.match(r.out, /brain not running — run `bellwether serve`/);
    assert.ok(!(r.out + r.err).includes(TOKEN));
  });

  test('timeouts: 600, missing, 0, a string and 2.5 all FAIL naming the event; 0.5 and 2 pass', () => {
    for (const bad of [600, undefined, 0, '1', 2.5, -1, null]) {
      const f = settingsWith((s) => { if (bad === undefined) delete s.hooks.Stop[0].hooks[0].timeout; else s.hooks.Stop[0].hooks[0].timeout = bad; });
      const c = check(doctor(f), 'hook timeout');
      assert.equal(c.status, 'FAIL', `timeout ${JSON.stringify(bad)}`);
      assert.match(c.detail, /Stop/);
    }
    for (const ok of [0.5, 2]) {
      const f = settingsWith((s) => { s.hooks.Stop[0].hooks[0].timeout = ok; });
      assert.equal(check(doctor(f), 'hook timeout').status, 'PASS');
    }
  });

  test('a timeout only wrong on a bellwether entry that shares a group with another tool is still found', () => {
    const f = settingsWith((s) => {
      s.hooks.Stop[0].hooks.unshift({ type: 'command', command: 'x', timeout: 600 });
      s.hooks.Stop[0].hooks[1].timeout = 99;
    });
    assert.equal(check(doctor(f), 'hook timeout').status, 'FAIL');
  });

  test('settings: missing file, invalid JSON, token mismatch, disableAllHooks, allowedHttpHookUrls', () => {
    const missing = check(doctor(path.join(root, 'nope.json')), 'hooks installed');
    assert.equal(missing.status, 'FAIL');
    assert.match(missing.detail, /no such file/);
    const bad = path.join(root, 'bad.json');
    fs.writeFileSync(bad, '{ nope');
    assert.match(check(doctor(bad), 'hooks installed').detail, /not valid JSON/);
    assert.equal(check(doctor(settingsWith((s) => { s.hooks.Stop[0].hooks[0].headers['X-Bellwether-Token'] = 'other'; })), 'hook token').status, 'FAIL');
    assert.equal(check(doctor(settingsWith((s) => { s.disableAllHooks = true; })), 'disableAllHooks').status, 'FAIL');
    assert.equal(check(doctor(settingsWith((s) => { s.allowedHttpHookUrls = ['https://example.com/*']; })), 'allowedHttpHookUrls').status, 'FAIL');
    assert.equal(check(doctor(settingsWith((s) => { s.allowedHttpHookUrls = []; })), 'allowedHttpHookUrls').status, 'FAIL');
  });

  test('missing token: exit 3 naming BELLWETHER_HOOK_TOKEN; short or odd token: exit 3', () => {
    const f = settingsWith(() => {});
    for (const t of [undefined, '', 'short', 'has space in it-----------', 'quote"quote_quote_quote']) {
      const e = { ...env };
      if (t === undefined) delete e.BELLWETHER_HOOK_TOKEN; else e.BELLWETHER_HOOK_TOKEN = t;
      const r = spawnBrainSync(['doctor', '--settings', f], { root, encoding: 'utf8', env: e });
      assert.equal(r.status, 3, JSON.stringify(t));
      assert.match(r.stdout, /BELLWETHER_HOOK_TOKEN/);
    }
  });

  test('missing/invalid config.json: names the file, exit 3, no stack', () => {
    fs.writeFileSync(path.join(root, 'config.json'), '{ "port": ');
    try {
      const r = cli(['doctor', '--settings', settingsPath]);
      assert.equal(r.code, 3);
      assert.match(r.out, /config\.json is not valid JSON/);
      assert.doesNotMatch(r.err, /at .*\.mjs/);
      fs.writeFileSync(path.join(root, 'config.json'), JSON.stringify({ port, bind: { Host: '0.0.0.0' } }));
      const h = cli(['doctor', '--settings', settingsPath]);
      assert.equal(h.code, 3);
      assert.match(h.out, /not configurable/);
      assert.equal(cli(['serve']).code, 3);
    } finally {
      fs.rmSync(path.join(root, 'config.json'));
    }
  });

  test('brain up with the right token: brain and brain token PASS, exit 0 on a full install', async () => {
    const b = await startBrain();
    try {
      const f = settingsWith(() => {});
      const r = doctor(f);
      assert.equal(r.code, 0, r.out);
      assert.equal(check(r, 'brain token').status, 'PASS');
    } finally {
      await b.stop();
    }
  });

  test('brain with a different token: FAIL "brain token" naming the cause', async () => {
    const b = await startBrain([], { BELLWETHER_HOOK_TOKEN: 'someOtherTokenValue1234567890' });
    try {
      const r = doctor(settingsWith(() => {}));
      const c = check(r, 'brain token');
      assert.equal(c.status, 'FAIL');
      assert.match(c.detail, /different BELLWETHER_HOOK_TOKEN/);
    } finally {
      await b.stop();
    }
  });

  test('brain hung (--hang): doctor says so rather than pass or crash', async () => {
    const b = await startBrain(['--hang']);
    try {
      const r = doctor(settingsWith(() => {}));
      assert.notEqual(r.code, 0);
      const brain = check(r, 'brain');
      const tok = check(r, 'brain token');
      const text = `${brain.detail} ${tok?.detail ?? ''}`;
      assert.match(text, /hang|didn't answer|timeout/i, text);
      assert.ok(brain.status === 'FAIL' || (tok && tok.status === 'FAIL'), 'a hung brain must FAIL somewhere');
    } finally {
      await b.stop();
    }
  });

  test('wrong thing on the port: an unrelated HTTP server, a raw TCP listener, and a silent listener', async () => {
    const servers = [
      http.createServer((q, s) => s.end('hello')),
      net.createServer((s) => s.end('not http at all\r\n')),
      net.createServer(() => {}),
    ];
    const names = ['http', 'tcp-garbage', 'silent'];
    for (let i = 0; i < servers.length; i++) {
      const socks = new Set();
      servers[i].on('connection', (k) => (socks.add(k), k.on('error', () => {})));
      await new Promise((r) => servers[i].listen(port, '127.0.0.1', r));
      try {
        const r = await cliAsync(['doctor', '--json', '--settings', settingsWith(() => {})]);
        const c = check(r, 'brain');
        assert.equal(c.status, 'FAIL', names[i]);
        console.log(`# ${names[i]} -> exit ${r.code}: ${c.detail}`);
        assert.equal(r.code, 2, names[i]);
        assert.doesNotMatch(c.detail, /undefined/);
      } finally {
        for (const k of socks) k.destroy();
        await new Promise((r) => servers[i].close(r));
      }
    }
  });

  test('a fake bellwether /health in front of a /hook that never answers is not reported as healthy', async () => {
    const s = http.createServer((q, res) => {
      if (q.url === '/health') {
        res.writeHead(200, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ ok: true, name: 'bellwether', pid: 1, version: 'x', counters: {} }));
      }
    });
    await new Promise((r) => s.listen(port, '127.0.0.1', r));
    try {
      const r = await cliAsync(['doctor', '--json', '--settings', settingsWith(() => {})]);
      assert.notEqual(r.code, 0);
      assert.equal(check(r, 'brain token')?.status, 'FAIL', r.out);
      console.log(`# fake-health -> exit ${r.code}: ${check(r, 'brain').status} ${check(r, 'brain').detail}; ${check(r, 'brain token').detail}`);
    } finally {
      s.closeAllConnections();
      await new Promise((r) => s.close(r));
    }
  });

  test('serving on a taken port fails with a message and exit 2, not a stack', async () => {
    const b = await startBrain();
    try {
      const r = cli(['serve']);
      assert.equal(r.code, 2);
      assert.match(r.err, /already in use/);
      assert.doesNotMatch(r.err, /at .*\.mjs/);
    } finally {
      await b.stop();
    }
  });
});

describe('fixtures and recordings', () => {
  test('committed fixtures contain no real session ids, terminal ids, home paths or user names', () => {
    // Every file, the shared vectors in installer-vectors/ included.
    const files = fs.readdirSync(FIXTURES, { recursive: true }).filter((f) => fs.statSync(path.join(FIXTURES, f)).isFile());
    assert.ok(files.some((f) => f.startsWith(`installer-vectors${path.sep}`)), 'the scan reaches into subdirectories');
    const blob = files.map((f) => fs.readFileSync(path.join(FIXTURES, f), 'utf8')).join('\n');
    // the only home directory is the invented one
    assert.doesNotMatch(blob, /\/Users\/(?!example\/)[^/\s"]+/);
    // every session id is one of the invented, patterned ones (the frozen differential
    // sample in frozen/ is generated: its ids are random words, never recorded ones)
    const recorded = files.filter((f) => !f.startsWith(`frozen${path.sep}`)).map((f) => fs.readFileSync(path.join(FIXTURES, f), 'utf8')).join('\n');
    for (const [, id] of recorded.matchAll(/"(?:session_id|sessionId)":"([^"]*)"/g)) assert.match(id, /^0f0e0d0c-\d{4}-4aaa-8bbb-0{9}\d{3}$/, id);
    for (const [, id] of recorded.matchAll(/"itermSession":"([^"]*)"/g)) assert.match(id, /^w\dt\dp\d:00000000-AAAA-4BBB-8CCC-0{9}\d{3}$/, id);
  });

  test('the fixture settings file holds only invented commands', () => {
    const blob = fs.readFileSync(path.join(FIXTURES, 'settings-sample.json'), 'utf8');
    assert.doesNotMatch(blob, /\/Users\/(?!example\/)[^/\s"]+/);
  });
});
