import assert from 'node:assert/strict';
import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';
import { describe, test } from 'node:test';
import { checkSettings, globMatch, nodeSupported, runDoctor } from '../brain/doctor.mjs';
import { hookBlock, install } from '../brain/hooks.mjs';
import { startServer } from '../brain/server.mjs';
import { EXIT } from '../brain/util.mjs';
import { TOKEN, tempDir } from './helpers.mjs';

/** Each failure cause the plan names gets a FAIL line with a cause, and the right exit code. */

const PORT = 47417;
const byName = (checks, name) => checks.find((c) => c.name === name);

describe('checkSettings', () => {
  test('a full install passes', () => {
    const checks = checkSettings(hookBlock(PORT, TOKEN), { port: PORT, token: TOKEN });
    assert.ok(checks.every((c) => c.status === 'PASS'), JSON.stringify(checks));
  });

  test('not installed: FAIL, exit 2', () => {
    const c = byName(checkSettings({ hooks: {} }, { port: PORT, token: TOKEN }), 'hooks installed');
    assert.equal(c.status, 'FAIL');
    assert.equal(c.exitCode, EXIT.NOT_RUNNING);
    assert.match(c.detail, /bellwether hooks install/);
  });

  test('a partial install names the missing events', () => {
    const s = hookBlock(PORT, TOKEN);
    delete s.hooks.Elicitation;
    const c = byName(checkSettings(s, { port: PORT, token: TOKEN }), 'hooks installed');
    assert.equal(c.status, 'FAIL');
    assert.match(c.detail, /15\/16 .*missing Elicitation/);
  });

  test('an entry without timeout, or above 2 s, fails', () => {
    for (const timeout of [undefined, 5, 600]) {
      const s = hookBlock(PORT, TOKEN);
      if (timeout === undefined) delete s.hooks.PreToolUse[0].hooks[0].timeout;
      else s.hooks.PreToolUse[0].hooks[0].timeout = timeout;
      const c = byName(checkSettings(s, { port: PORT, token: TOKEN }), 'hook timeout');
      assert.equal(c.status, 'FAIL');
      assert.match(c.detail, /PreToolUse/);
    }
  });

  test('a token that differs from .env fails', () => {
    const c = byName(checkSettings(hookBlock(PORT, 'someOtherTokenValue1'), { port: PORT, token: TOKEN }), 'hook token');
    assert.equal(c.status, 'FAIL');
    assert.match(c.detail, /differs from BELLWETHER_HOOK_TOKEN/);
  });

  test('disableAllHooks: true fails', () => {
    const c = byName(checkSettings({ ...hookBlock(PORT, TOKEN), disableAllHooks: true }, { port: PORT, token: TOKEN }), 'disableAllHooks');
    assert.equal(c.status, 'FAIL');
  });

  test('allowedHttpHookUrls without bellwether fails; with a matching pattern passes', () => {
    const without = byName(checkSettings({ ...hookBlock(PORT, TOKEN), allowedHttpHookUrls: ['https://example.com/*'] }, { port: PORT, token: TOKEN }), 'allowedHttpHookUrls');
    assert.equal(without.status, 'FAIL');
    const withIt = byName(checkSettings({ ...hookBlock(PORT, TOKEN), allowedHttpHookUrls: ['http://127.0.0.1:*'] }, { port: PORT, token: TOKEN }), 'allowedHttpHookUrls');
    assert.equal(withIt.status, 'PASS');
  });

  test('install on top of realistic settings passes', () => {
    const s = install({ hooks: { Stop: [{ hooks: [{ type: 'command', command: 'x' }] }] } }, PORT, TOKEN);
    assert.equal(byName(checkSettings(s, { port: PORT, token: TOKEN }), 'hooks installed').status, 'PASS');
  });

  test('an entry on SessionStart is flagged as never firing', () => {
    const s = hookBlock(PORT, TOKEN);
    s.hooks.SessionStart = [{ hooks: [structuredClone(s.hooks.Stop[0].hooks[0])] }];
    const c = byName(checkSettings(s, { port: PORT, token: TOKEN }), 'hooks unsupported');
    assert.equal(c.status, 'WARN');
    assert.match(c.detail, /SessionStart never fire/);
  });

  test('a group whose hooks is not an array: FAIL "settings shape", exit 3, no exception', () => {
    for (const bad of ['text', {}, 5, null]) {
      const checks = checkSettings({ hooks: { Stop: [{ hooks: bad }] } }, { port: PORT, token: TOKEN });
      const c = byName(checks, 'settings shape');
      assert.equal(c.status, 'FAIL');
      assert.equal(c.exitCode, EXIT.BAD_CONFIG);
      assert.match(c.detail, /hooks\.Stop\[0\]\.hooks/);
    }
  });

  test('Node 22.9 is the floor (--env-file-if-exists)', () => {
    assert.ok(!nodeSupported('22.8.0'));
    assert.ok(!nodeSupported('20.19.0'));
    assert.ok(nodeSupported('22.9.0'));
    assert.ok(nodeSupported('v24.1.0'));
  });

  test('globMatch', () => {
    assert.ok(globMatch('http://127.0.0.1:47417/hook', 'http://127.0.0.1:47417/hook'));
    assert.ok(globMatch('http://127.0.0.1:*', 'http://127.0.0.1:47417/hook'));
    assert.ok(!globMatch('http://127.0.0.1:47417', 'http://127.0.0.1:47417/hook'));
  });
});

describe('runDoctor', () => {
  const settingsWith = (obj) => {
    const file = path.join(tempDir(), 'settings.json');
    fs.writeFileSync(file, JSON.stringify(obj));
    return file;
  };
  const env = { BELLWETHER_HOOK_TOKEN: TOKEN };

  test('brain down: exit 2 with "brain not running — run `bellwether serve`"', async () => {
    // Bind and release a port so nothing is listening on it.
    const probe = http.createServer();
    await new Promise((r) => probe.listen(0, '127.0.0.1', r));
    const port = probe.address().port;
    await new Promise((r) => probe.close(r));
    const r = await runDoctor({ env, settingsFile: settingsWith(hookBlock(port, TOKEN)), configLoader: () => ({ config: { port }, usingExample: false }) });
    const brain = byName(r.checks, 'brain');
    assert.equal(brain.status, 'FAIL');
    assert.match(brain.detail, /^brain not running — run `bellwether serve`/);
    assert.equal(r.exitCode, EXIT.NOT_RUNNING);
    assert.match(byName(r.checks, 'hook errors').detail, /visible .*hook error/);
  });

  test('brain up with the same token: everything passes, exit 0', async () => {
    const brain = await startServer({ port: 0, token: TOKEN });
    try {
      const r = await runDoctor({ env, settingsFile: settingsWith(hookBlock(brain.port, TOKEN)), configLoader: () => ({ config: { port: brain.port }, usingExample: false }) });
      assert.equal(r.exitCode, EXIT.OK, JSON.stringify(r.checks, null, 2));
      assert.equal(byName(r.checks, 'brain token').status, 'PASS');
    } finally {
      await brain.close();
    }
  });

  test('brain started with another token: FAIL brain token, exit 3', async () => {
    const brain = await startServer({ port: 0, token: 'aDifferentTokenValue123' });
    try {
      const r = await runDoctor({ env, settingsFile: settingsWith(hookBlock(brain.port, TOKEN)), configLoader: () => ({ config: { port: brain.port }, usingExample: false }) });
      assert.equal(byName(r.checks, 'brain token').status, 'FAIL');
      assert.equal(r.exitCode, EXIT.BAD_CONFIG);
    } finally {
      await brain.close();
    }
  });

  test('port taken by another process: FAIL naming it', async () => {
    const other = http.createServer((req, res) => res.end('hello from something else'));
    await new Promise((r) => other.listen(0, '127.0.0.1', r));
    const port = other.address().port;
    try {
      const r = await runDoctor({ env, settingsFile: settingsWith(hookBlock(port, TOKEN)), configLoader: () => ({ config: { port }, usingExample: false }) });
      const brain = byName(r.checks, 'brain');
      assert.equal(brain.status, 'FAIL');
      assert.match(brain.detail, new RegExp(`port ${port} is held by another process \\(pid ${process.pid}`));
      assert.equal(r.exitCode, EXIT.NOT_RUNNING);
    } finally {
      await new Promise((r) => other.close(r));
    }
  });

  test('a published example token is refused, exit 3', async () => {
    const r = await runDoctor({ env: { BELLWETHER_HOOK_TOKEN: 'q3V0mZ8x2k1rT9bN4cW7yL5pA6sD0fGh' }, settingsFile: settingsWith({}), configLoader: () => ({ config: { port: 1 }, usingExample: false }) });
    const t = byName(r.checks, 'token');
    assert.equal(t.status, 'FAIL');
    assert.match(t.detail, /published example/);
    assert.equal(r.exitCode, EXIT.BAD_CONFIG);
  });

  test('brain in --hang mode: FAIL brain naming --hang, exit 2', async () => {
    const brain = await startServer({ port: 0, token: TOKEN, hang: true });
    try {
      const r = await runDoctor({ env, settingsFile: settingsWith(hookBlock(brain.port, TOKEN)), configLoader: () => ({ config: { port: brain.port }, usingExample: false }) });
      const c = byName(r.checks, 'brain');
      assert.equal(c.status, 'FAIL');
      assert.match(c.detail, /--hang/);
      assert.equal(r.exitCode, EXIT.NOT_RUNNING);
      assert.equal(byName(r.checks, 'hook errors'), undefined, 'a hung brain prints no ECONNREFUSED errors');
    } finally {
      await brain.close();
    }
  });

  test('missing token: FAIL with how to generate it, exit 3', async () => {
    const r = await runDoctor({ env: {}, settingsFile: settingsWith({}), configLoader: () => ({ config: { port: 1 }, usingExample: false }) });
    const t = byName(r.checks, 'token');
    assert.equal(t.status, 'FAIL');
    assert.match(t.detail, /bellwether token init/);
    assert.equal(r.exitCode, EXIT.BAD_CONFIG);
  });

  test('a settings file that is not JSON: FAIL, exit 3', async () => {
    const file = path.join(tempDir(), 'settings.json');
    fs.writeFileSync(file, '{nope');
    const r = await runDoctor({ env, settingsFile: file, configLoader: () => ({ config: { port: 1 }, usingExample: false }) });
    assert.match(byName(r.checks, 'hooks installed').detail, /not valid JSON/);
    assert.equal(r.exitCode, EXIT.BAD_CONFIG);
  });
});

describe('other settings levels (project, local, managed)', async () => {
  const { checkOtherLevels } = await import('../brain/doctor.mjs');
  const project = (files) => {
    const dir = tempDir();
    fs.mkdirSync(path.join(dir, '.claude'));
    for (const [name, v] of Object.entries(files)) fs.writeFileSync(path.join(dir, '.claude', name), typeof v === 'string' ? v : JSON.stringify(v));
    return dir;
  };
  const managed = (v) => {
    const f = path.join(tempDir(), 'managed-settings.json');
    fs.writeFileSync(f, JSON.stringify(v));
    return f;
  };
  const by = (checks, name) => checks.find((c) => c.name === name);

  test('nothing found: one INFO line, no failures', () => {
    const checks = checkOtherLevels({ projectDir: tempDir(), managedFile: path.join(tempDir(), 'none.json'), port: PORT });
    assert.deepEqual(checks.map((c) => c.status), ['INFO']);
  });
  test('project disableAllHooks: FAIL naming the file', () => {
    const c = by(checkOtherLevels({ projectDir: project({ 'settings.json': { disableAllHooks: true } }), port: PORT }), 'project settings');
    assert.equal(c.status, 'FAIL');
    assert.match(c.detail, /disableAllHooks/);
  });
  test('local allowedHttpHookUrls without bellwether: FAIL on the merged list; with it: no FAIL', () => {
    assert.equal(by(checkOtherLevels({ projectDir: project({ 'settings.local.json': { allowedHttpHookUrls: ['https://ci.example/*'] } }), port: PORT }), 'allowedHttpHookUrls (merged)').status, 'FAIL');
    const ok = checkOtherLevels({ projectDir: project({ 'settings.local.json': { allowedHttpHookUrls: ['https://ci.example/*'] }, 'settings.json': { allowedHttpHookUrls: ['http://127.0.0.1:*/hook'] } }), port: PORT });
    assert.equal(by(ok, 'allowedHttpHookUrls (merged)'), undefined);
  });
  test('managed allowManagedHooksOnly: FAIL', () => {
    const c = by(checkOtherLevels({ managedFile: managed({ allowManagedHooksOnly: true }), port: PORT }), 'managed settings');
    assert.equal(c.status, 'FAIL');
    assert.match(c.detail, /allowManagedHooksOnly/);
  });
  test('httpHookAllowedEnvVars missing the terminal variables: WARN', () => {
    const c = by(checkOtherLevels({ projectDir: project({ 'settings.json': { httpHookAllowedEnvVars: ['TERM_PROGRAM'] } }), port: PORT }), 'httpHookAllowedEnvVars (merged)');
    assert.equal(c.status, 'WARN');
    assert.match(c.detail, /HERDR_PANE_ID/);
  });
  test('a project settings file that is not JSON: WARN, not a crash', () => {
    assert.equal(by(checkOtherLevels({ projectDir: project({ 'settings.json': '{nope' }), port: PORT }), 'project settings').status, 'WARN');
  });
});

describe('doctor --e2e (fake claude)', () => {
  const settingsWith = (obj) => {
    const file = path.join(tempDir(), 'settings.json');
    fs.writeFileSync(file, JSON.stringify(obj));
    return file;
  };
  const env = { BELLWETHER_HOOK_TOKEN: TOKEN };
  const postHook = (port, body) =>
    new Promise((res) => {
      const data = JSON.stringify(body);
      const q = http.request({ host: '127.0.0.1', port, method: 'POST', path: '/hook', headers: { 'Content-Type': 'application/json', 'X-Bellwether-Token': TOKEN, 'Content-Length': Buffer.byteLength(data) } }, (r) => (r.resume(), r.on('end', res)));
      q.end(data);
    });

  test('PASS when the run delivers its events; the hooks file it used holds bellwether\'s block', async () => {
    const brain = await startServer({ port: 0, token: TOKEN });
    try {
      let sawHooks = null;
      const runClaude = async ({ hooksFile, cwd }) => {
        sawHooks = JSON.parse(fs.readFileSync(hooksFile, 'utf8'));
        assert.ok(fs.existsSync(cwd));
        for (const e of ['UserPromptSubmit', 'Stop', 'SessionEnd']) await postHook(brain.port, { hook_event_name: e, session_id: 'e2e' });
        return { code: 0, stderr: '' };
      };
      const r = await runDoctor({ env, e2e: true, runClaude, settingsFile: settingsWith(hookBlock(brain.port, TOKEN)), configLoader: () => ({ config: { port: brain.port }, usingExample: false }) });
      const c = byName(r.checks, 'e2e');
      assert.equal(c.status, 'PASS', c.detail);
      assert.match(c.detail, /delivered 3 hook events/);
      assert.deepEqual(sawHooks, hookBlock(brain.port, TOKEN));
    } finally {
      await brain.close();
    }
  });

  test('FAIL when claude runs but no events arrive, or claude is missing', async () => {
    const brain = await startServer({ port: 0, token: TOKEN });
    try {
      const base = { env, e2e: true, settingsFile: settingsWith(hookBlock(brain.port, TOKEN)), configLoader: () => ({ config: { port: brain.port }, usingExample: false }) };
      const silent = await runDoctor({ ...base, runClaude: async () => ({ code: 0, stderr: '' }) });
      assert.match(byName(silent.checks, 'e2e').detail, /saw only 0 new events/);
      const missing = await runDoctor({ ...base, runClaude: async () => ({ error: 'claude is not on PATH' }) });
      assert.match(byName(missing.checks, 'e2e').detail, /claude is not on PATH/);
      assert.notEqual(missing.exitCode, 0);
    } finally {
      await brain.close();
    }
  });

  test('skipped with a FAIL when the brain is not running', async () => {
    const r = await runDoctor({ env, e2e: true, runClaude: async () => assert.fail('must not run claude'), settingsFile: settingsWith({}), configLoader: () => ({ config: { port: 1 }, usingExample: false }) });
    assert.match(byName(r.checks, 'e2e').detail, /needs a running brain/);
  });
});

describe('doctor wording for entries that are not quite bellwether\'s', () => {
  test('a bellwether-style URL on another port: "bellwether-like" only with X-Bellwether-Token, else another tool\'s hook', () => {
    const withToken = { type: 'http', url: 'http://127.0.0.1:47500/hook', headers: { 'X-Bellwether-Token': 'x' } };
    const without = { type: 'http', url: 'http://127.0.0.1:47501/hook' };
    const checks = checkSettings({ hooks: { Stop: [{ hooks: [withToken, without] }] } }, { port: PORT, token: TOKEN });
    assert.match(byName(checks, 'hooks stray').detail, /bellwether-like .*47500/);
    assert.doesNotMatch(byName(checks, 'hooks stray').detail, /47501/);
    assert.match(byName(checks, 'hooks foreign').detail, /another tool's hook .*47501/);
  });

  test('bellwether\'s exact URL without the token header is flagged as probably another tool\'s', () => {
    const block = hookBlock(PORT, TOKEN);
    delete block.hooks.Stop[0].hooks[0].headers['X-Bellwether-Token'];
    const c = checkSettings(block, { port: PORT, token: TOKEN }).find((x) => x.name === 'hooks foreign');
    assert.equal(c.status, 'WARN');
    assert.match(c.detail, /another tool's hook on bellwether's port/);
  });

  test('--e2e says why claude -p failed, redacted', async () => {
    const brain = await startServer({ port: 0, token: TOKEN });
    try {
      const file = path.join(tempDir(), 'settings.json');
      fs.writeFileSync(file, JSON.stringify(hookBlock(brain.port, TOKEN)));
      const r = await runDoctor({
        env: { BELLWETHER_HOOK_TOKEN: TOKEN },
        e2e: true,
        settingsFile: file,
        configLoader: () => ({ config: { port: brain.port }, usingExample: false }),
        runClaude: async () => ({ code: 1, stderr: 'starting\nusing ANTHROPIC_API_KEY=sk-abcdefghijklmnopqrstuvwx\nInvalid API key · Please run /login' }),
      });
      const c = r.checks.find((x) => x.name === 'e2e');
      assert.equal(c.status, 'FAIL');
      assert.match(c.detail, /exited 1: .*Invalid API key · Please run \/login/);
      assert.doesNotMatch(c.detail, /abcdefghijklmnop/);
    } finally {
      await brain.close();
    }
  });
});
