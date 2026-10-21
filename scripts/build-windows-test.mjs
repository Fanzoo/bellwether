#!/usr/bin/env node
/**
 * Builds Bellwether's Windows test folder (Windows plan W1, WD12): what the maintainer
 * unzips on his Windows PC for the by-hand checks, until the installer exists (W8). CI's
 * `windows` job uploads it as the `bellwether-windows-test` artifact (a .zip).
 *
 *   node scripts/build-windows-test.mjs          on Windows (CI): `cargo build --release`
 *   node scripts/build-windows-test.mjs --xwin   on a Mac: cross-built with cargo-xwin
 *        (`cargo install cargo-xwin`, and LLVM's clang-cl, llvm-lib and lld-link on PATH:
 *        `brew install llvm lld`, then PATH="$(brew --prefix llvm)/bin:$(brew --prefix lld)/bin:$PATH")
 *   --out <dir>   where the folder goes (default shell/src-tauri/target-dev/windows-test/bellwether-windows-test)
 *
 * The folder:
 *   Bellwether.exe   the unsigned release-profile exe with `--features test-build` (it
 *                    answers --dump-desktop; a release never does)
 *   run-test.cmd     the only way to start it: a scratch home beside it, the test port,
 *                    and `--demo`, so it never reads or writes %APPDATA%\Bellwether,
 *                    %USERPROFILE%\.claude or port 47417, and never reaches the CLI
 *   config.json      config.example.json on the test port (TEST_PORT)
 *   pets\<name>\animations.xml   Bellwether's own pets (checked against the pins compiled in)
 *   README.txt
 *
 * The exe runs in the repository layout (it is not inside an installed app): run-test.cmd
 * points BELLWETHER_AGENT_DIR at the folder and BELLWETHER_HOME at `home\` in it. A test
 * port and home compiled into the exe (the Windows form of build-app.mjs --test-port and
 * --test-home) come with W2, when the zip gets the brain and the CLI (deviation W1-3).
 */
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const AGENT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const TAURI_DIR = path.join(AGENT, 'shell', 'src-tauri');
const TARGET_DEV = path.join(TAURI_DIR, 'target-dev');
const TRIPLE = 'x86_64-pc-windows-msvc';

/** The test folder's port: never the live brain's 47417. */
export const TEST_PORT = 47491;
/** Bellwether's own pets (crates/brain/src/pets.rs BUNDLED). */
export const PETS = ['bellwether', 'brah'];

/** config.example.json with the test port, every other line as it is. */
export function testConfig(example, port = TEST_PORT) {
  const out = String(example).replace(/^(\s*"port":\s*)\d+/m, `$1${port}`);
  if (JSON.parse(out).port !== port) throw new Error('config.example.json has no top-level "port" line');
  return out;
}

/** run-test.cmd, with CRLF line endings (cmd reads labels and blocks line by line). */
export function runTestCmd(port = TEST_PORT) {
  const lines = [
    '@echo off',
    'rem Bellwether test build (Windows plan W1). Never the installed app: a scratch home',
    'rem beside this file, test port ' + port + ', and the demo flock (no brain, no hooks).',
    '',
    'rem   run-test.cmd                           the demo flock on every monitor, with the',
    'rem                                          heartbeat in the log; quits after 30 minutes',
    'rem   run-test.cmd --quit-after 900          ... after 15 minutes instead',
    'rem   run-test.cmd --dump-desktop desktop.json   the monitors and windows, as a file',
    'setlocal',
    'set "HERE=%~dp0"',
    'set "BELLWETHER_AGENT_DIR=%HERE:~0,-1%"',
    'set "BELLWETHER_HOME=%HERE%home"',
    'set "BELLWETHER_CONFIG="',
    'set "BELLWETHER_DATA_DIR="',
    'set "BELLWETHER_HOOK_TOKEN="',
    'if not exist "%BELLWETHER_HOME%" mkdir "%BELLWETHER_HOME%"',
    'if /i "%~1"=="--dump-desktop" goto dump',
    'rem Each run starts a fresh log; the one before is kept as shell.previous.log.',
    'if exist "%BELLWETHER_HOME%\\shell.log" move /y "%BELLWETHER_HOME%\\shell.log" "%BELLWETHER_HOME%\\shell.previous.log" >nul',
    'echo Starting the demo flock on every monitor (test port ' + port + ').',
    'echo Log: %BELLWETHER_HOME%\\shell.log',
    'echo Quit: the sheep icon in the notification area (it may be under ^^), Quit Bellwether.',
    'start "" /wait "%HERE%Bellwether.exe" --demo --demo-monitors --heartbeat %* --quit-after 1800',
    'echo Stopped. Log: %BELLWETHER_HOME%\\shell.log',
    'goto :eof',
    ':dump',
    'if "%~2"=="" (',
    '  echo usage: run-test.cmd --dump-desktop desktop.json',
    '  exit /b 2',
    ')',
    'if exist "%~f2" del "%~f2"',
    'start "" /wait "%HERE%Bellwether.exe" --dump-desktop "%~f2"',
    'set "RC=%ERRORLEVEL%"',
    'if not "%RC%"=="0" goto dumpfailed',
    'if not exist "%~f2" (',
    '  set "RC=1"',
    '  goto dumpfailed',
    ')',
    'echo Wrote %~f2',
    'goto :eof',
    ':dumpfailed',
    'echo Could not write %~f2 (exit code %RC%). The cause is in the --dump-desktop lines of %BELLWETHER_HOME%\\shell.log:',
    'findstr /c:"--dump-desktop:" "%BELLWETHER_HOME%\\shell.log"',
    'exit /b %RC%',
  ];
  return lines.join('\r\n') + '\r\n';
}

export function readme(port = TEST_PORT) {
  return [
    'Bellwether: Windows test build (unsigned; never ship it)',
    '',
    'Start it with run-test.cmd, from Windows Terminal or by double-clicking. It shows the',
    'demo flock (one sheep per behaviour) on every monitor, standing on the taskbar. It runs',
    `on test port ${port} with its own home folder, home\\, next to this file. It never reads`,
    'or writes %APPDATA%\\Bellwether or %USERPROFILE%\\.claude, never uses port 47417, and',
    'starts no brain and no hooks.',
    '',
    '  run-test.cmd                              the demo; quits after 30 minutes',
    '  run-test.cmd --quit-after 900             quits after 15 minutes instead',
    '  run-test.cmd --dump-desktop desktop.json  writes the monitors and windows to a file',
    '',
    'Quit early: the sheep icon in the notification area (it may be under the ^), Quit',
    'Bellwether. The log is home\\shell.log. Each run-test.cmd demo run starts a fresh one',
    '(the run before is kept as home\\shell.previous.log), so attach shell.log after the run',
    'you are reporting on. --dump-desktop adds its outcome to the current shell.log, and',
    'exits with a non-zero code if the file could not be written.',
    '',
    'Windows may warn that the app is unrecognised (it is unsigned): More info, Run anyway.',
    '',
    'Cleaning up: delete this folder, and %LOCALAPPDATA%\\local.bellwether.p' + port,
    '(WebView2\'s files for this test build).',
    '',
  ].join('\r\n');
}

function fail(message) {
  process.stderr.write(`build-windows-test: ${message}\n`);
  process.exit(1);
}

function parse(argv) {
  const o = { xwin: false, out: path.join(TARGET_DEV, 'windows-test', 'bellwether-windows-test') };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === '--xwin') o.xwin = true;
    else if (a === '--out') o.out = path.resolve(argv[++i] ?? fail('--out needs a directory'));
    else fail(`unknown option ${JSON.stringify(a)} (see the comment at the top of scripts/build-windows-test.mjs)`);
  }
  if (!o.xwin && process.platform !== 'win32') fail('this builds a Windows exe: run it on Windows, or with --xwin on a Mac');
  return o;
}

function build(o) {
  const manifest = path.join(TAURI_DIR, 'Cargo.toml');
  const args = ['build', '--release', '--locked', '--manifest-path', manifest, '--features', 'test-build'];
  const [cmd, full] = o.xwin ? ['cargo', ['xwin', ...args, '--target', TRIPLE]] : ['cargo', args];
  process.stdout.write(`$ ${cmd} ${full.join(' ')}\n`);
  execFileSync(cmd, full, { stdio: 'inherit', cwd: AGENT });
  const exe = path.join(TARGET_DEV, ...(o.xwin ? [TRIPLE] : []), 'release', 'bellwether-shell.exe');
  if (!fs.existsSync(exe)) fail(`${exe} is missing after the build`);
  return exe;
}

function assemble(exe, out) {
  fs.rmSync(out, { recursive: true, force: true });
  fs.mkdirSync(out, { recursive: true });
  fs.copyFileSync(exe, path.join(out, 'Bellwether.exe'));
  fs.writeFileSync(path.join(out, 'run-test.cmd'), runTestCmd());
  fs.writeFileSync(path.join(out, 'config.json'), testConfig(fs.readFileSync(path.join(AGENT, 'config.example.json'), 'utf8')));
  fs.writeFileSync(path.join(out, 'README.txt'), readme());
  for (const pet of PETS) {
    const from = path.join(AGENT, 'pets', pet, 'animations.xml');
    if (!fs.existsSync(from)) fail(`${from} is missing: build it with \`node scripts/build-pet.mjs build ${pet}\``);
    fs.mkdirSync(path.join(out, 'pets', pet), { recursive: true });
    fs.copyFileSync(from, path.join(out, 'pets', pet, 'animations.xml'));
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const o = parse(process.argv.slice(2));
  const exe = build(o);
  assemble(exe, o.out);
  const files = fs.readdirSync(o.out).sort();
  process.stdout.write(`files: ${files.join(', ')}\n`);
  process.stdout.write(`test build (unsigned, never ship one), port ${TEST_PORT}: ${o.out}\n`);
}
