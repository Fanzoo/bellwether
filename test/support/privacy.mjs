/**
 * A test driver, copied from the Node brain at v0.2.0-rc.3 (brain/privacy.mjs): writes the
 * strip's switches the way the shell does, for tests that read them back through the Rust
 * brain. Not the brain: no test checks it as one.
 */
import fs from 'node:fs';
import path from 'node:path';

/** Writes the switches the way the shell does (a temporary file, then a rename). */
export function writeShellState(dataDir, { hidden, showLabels }) {
  fs.mkdirSync(dataDir, { recursive: true });
  const tmp = path.join(dataDir, 'shell-state.json.tmp');
  fs.writeFileSync(tmp, JSON.stringify({ version: 1, hidden: !!hidden, showLabels: !!showLabels }, null, 2));
  fs.renameSync(tmp, path.join(dataDir, 'shell-state.json'));
}
