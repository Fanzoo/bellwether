import fs from 'node:fs';
import { envFilePath } from './envpath.mjs';

/**
 * Imported first by cli.mjs: loads the `.env` (envpath.mjs says which) into
 * process.env before anything reads it. Like `node --env-file`, a variable that is
 * already set, even to "", wins over the file.
 */
const file = envFilePath();
if (fs.existsSync(file)) {
  try {
    process.loadEnvFile(file);
  } catch (e) {
    process.stderr.write(`pasture: could not read ${file} (${e.code ?? e.message})\n`);
    process.exit(3);
  }
}
