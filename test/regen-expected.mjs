/**
 * Regenerates test/fixtures/*.expected.jsonl from the reducer. The expected files are
 * golden files: after regenerating, read the diff and check every changed transition
 * against the state model and clearing rules (crates/core flock.rs) before committing it.
 *
 *   node test/regen-expected.mjs
 */
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { readJsonl, runFixture } from './fixture-runner.mjs';

const DIR = path.join(path.dirname(fileURLToPath(import.meta.url)), 'fixtures');
for (const f of fs.readdirSync(DIR).filter((f) => /^s\d+-.*\.jsonl$/.test(f) && !f.includes('.expected.'))) {
  const { transitions } = runFixture(readJsonl(path.join(DIR, f)));
  const out = path.join(DIR, f.replace(/\.jsonl$/, '.expected.jsonl'));
  fs.writeFileSync(out, transitions.map((t) => JSON.stringify(t)).join('\n') + '\n');
  console.log(`${path.basename(out)}: ${transitions.length} transitions`);
}
