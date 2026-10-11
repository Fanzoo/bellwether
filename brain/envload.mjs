import fs from 'node:fs';
import { adoptLegacyEnv, envFilePath } from './envpath.mjs';

/**
 * Imported first by cli.mjs: loads the `.env` (envpath.mjs says which) into
 * process.env before anything reads it. Like `node --env-file`, a variable that is
 * already set, even to "", wins over the file. Old PASTURE_* names are adopted only
 * after the file is loaded, so the order is: an exported BELLWETHER_*, then
 * BELLWETHER_* in the .env, then an exported PASTURE_*, then PASTURE_* in the .env.
 */
const file = envFilePath();
if (fs.existsSync(file)) {
  try {
    process.loadEnvFile(file);
  } catch (e) {
    process.stderr.write(`bellwether: could not read ${file} (${e.code ?? e.message})\n`);
    process.exit(3);
  }
}
adoptLegacyEnv();
