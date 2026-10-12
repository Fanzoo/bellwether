import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

/**
 * Where cargo is, for building the strip: on PATH, then ~/.cargo/bin, then
 * $CARGO_HOME/bin (rustup installs there, but a shell that never sourced
 * ~/.cargo/env, or a LaunchAgent, doesn't have it on PATH). Null when there is none.
 */
export function findCargo({ env = process.env, home = os.homedir(), platform = process.platform } = {}) {
  const exe = platform === 'win32' ? 'cargo.exe' : 'cargo';
  const dirs = [...(env.PATH ?? '').split(path.delimiter).filter(Boolean), path.join(home, '.cargo', 'bin'), ...(env.CARGO_HOME ? [path.join(env.CARGO_HOME, 'bin')] : [])];
  for (const d of dirs) {
    const f = path.join(d, exe);
    try {
      if (fs.statSync(f).isFile()) {
        fs.accessSync(f, fs.constants.X_OK);
        return f;
      }
    } catch {}
  }
  return null;
}

export const CARGO_FIX = "cargo (Rust) wasn't found on PATH, in ~/.cargo/bin or in $CARGO_HOME/bin: run `source ~/.cargo/env`, or install rustup from https://rustup.rs";

/** `cargo --version`, for doctor. */
export function cargoVersion(cargo) {
  try {
    return execFileSync(cargo, ['--version'], { encoding: 'utf8', timeout: 5000, stdio: ['ignore', 'pipe', 'ignore'] }).trim();
  } catch {
    return null;
  }
}
