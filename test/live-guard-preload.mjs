// Preloaded (node --import) into the test processes under test/tester-live-guard.test.mjs.
// Records every attempt to reach the maintainer's live brain (port 47417) or launchctl/lsof,
// without stopping the test, so the guard test can list them.
import cp from 'node:child_process';
import fs from 'node:fs';
import net from 'node:net';
import { syncBuiltinESMExports } from 'node:module';

const log = process.env.LIVE_GUARD_LOG;
const note = (what) => log && fs.appendFileSync(log, `${what}\n`);
const LIVE = '47417';

const connect = net.Socket.prototype.connect;
net.Socket.prototype.connect = function (...args) {
  const a = args[0];
  const first = Array.isArray(a) ? a[0] : a; // net.connect() hands over its normalised [options, callback]
  const port = typeof first === 'object' && first !== null ? first.port : first;
  if (String(port) === LIVE) note(`socket to port ${LIVE}: ${new Error().stack.split('\n').slice(2, 6).join(' <- ').replace(/\s+/g, ' ')}`);
  return connect.apply(this, args);
};

for (const name of ['spawnSync', 'execFileSync', 'spawn', 'execFile', 'execSync', 'exec']) {
  const orig = cp[name];
  cp[name] = function (cmd, args, ...rest) {
    const argv = Array.isArray(args) ? args.map(String) : [];
    const text = `${cmd} ${argv.join(' ')}`;
    if (/(^|\/)launchctl$/.test(String(cmd).split(' ')[0]) || /(^|\/)lsof$/.test(String(cmd).split(' ')[0])) note(`${name}: ${text.slice(0, 160)}`);
    else if (text.includes(LIVE)) note(`${name} mentions port ${LIVE}: ${text.slice(0, 160)}`);
    return orig.call(this, cmd, args, ...rest);
  };
}
syncBuiltinESMExports();
