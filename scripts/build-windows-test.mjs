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
 *   bin\bellwether.exe   the console CLI (W2, WD7; `--features console-cli`), the same build
 *   run-test.cmd     the only way to start the strip: a scratch home beside it, the test
 *                    port, and `--demo`, or `--sessions` (W2: real Claude Code sessions,
 *                    with the strip's own brain), so it never reads or writes
 *                    %APPDATA%\Bellwether, %USERPROFILE%\.claude or port 47417
 *   cli.cmd          the CLI with the same home and config: `.\cli doctor` (not
 *                    bellwether.cmd: beside Bellwether.exe, `.\bellwether` runs the exe,
 *                    since cmd tries .EXE before .CMD and NTFS ignores case)
 *   config.json      config.example.json on the test port (TEST_PORT)
 *   pets\<name>\animations.xml   Bellwether's own pets (checked against the pins compiled in)
 *   README.txt
 *
 * The exes run in the repository layout (they are not inside an installed app): the two
 * .cmd files point BELLWETHER_AGENT_DIR at the folder and BELLWETHER_HOME at `home\` in it
 * (deviation W1-3). The test build itself refuses what would reach the live install
 * (cli.rs `test_build_refusal`, deviation W2-2): `hooks install|uninstall`, `token rotate`
 * and `migrate` without --settings, `autostart install|uninstall`, and port 47417.
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
export const PETS = ['bellwether', 'brah', 'bryan'];

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
    'rem Bellwether test build (Windows plan W1, W2). Never the installed app: a scratch home',
    'rem beside this file, test port ' + port + '.',
    '',
    'rem   run-test.cmd                           the demo flock on every monitor, with the',
    'rem                                          heartbeat in the log; quits after 30 minutes',
    'rem   run-test.cmd --quit-after 900          ... after 15 minutes instead',
    'rem   run-test.cmd --sessions                real Claude Code sessions: the strip and its',
    'rem                                          brain; then claude --settings hooks.json',
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
    'if /i "%~1"=="--sessions" goto sessions',
    'rem Each run starts a fresh log; the one before is kept as shell.previous.log.',
    'if exist "%BELLWETHER_HOME%\\shell.log" move /y "%BELLWETHER_HOME%\\shell.log" "%BELLWETHER_HOME%\\shell.previous.log" >nul',
    'echo Starting the demo flock on every monitor (test port ' + port + ').',
    'echo Log: %BELLWETHER_HOME%\\shell.log',
    'echo Quit: the sheep icon in the notification area (it may be under ^^), Quit Bellwether.',
    'start "" /wait "%HERE%Bellwether.exe" --demo --demo-monitors --heartbeat %* --quit-after 1800',
    'echo Stopped. Log: %BELLWETHER_HOME%\\shell.log',
    'goto :eof',
    ':sessions',
    'rem The hook token (made once, kept in home\\hook-token), and the hooks as a file for',
    'rem claude --settings: your own settings.json is never touched.',
    'call "%HERE%cli.cmd" token init',
    'if errorlevel 1 exit /b 1',
    'call "%HERE%cli.cmd" hooks print --out "%HERE%hooks.json"',
    'if errorlevel 1 exit /b 1',
    'if exist "%BELLWETHER_HOME%\\shell.log" move /y "%BELLWETHER_HOME%\\shell.log" "%BELLWETHER_HOME%\\shell.previous.log" >nul',
    'echo Starting the strip and its brain for real sessions (test port ' + port + ').',
    'echo In another terminal (a Windows Terminal tab, or a herdr pane), start Claude Code with:',
    'echo   claude --settings "%HERE%hooks.json"',
    'echo Logs: %BELLWETHER_HOME%\\shell.log and %BELLWETHER_HOME%\\logs\\brain.log',
    'echo Quit: the sheep icon in the notification area (it may be under ^^), Quit Bellwether.',
    'start "" /wait "%HERE%Bellwether.exe"',
    'echo Stopped. Logs: %BELLWETHER_HOME%\\shell.log and %BELLWETHER_HOME%\\logs\\brain.log',
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

/** cli.cmd: the console CLI with the folder's home and config, CRLF. */
export function cliCmd(port = TEST_PORT) {
  const lines = [
    '@echo off',
    'rem The Bellwether test build\'s command line (Windows plan W2): bin\\bellwether.exe with',
    'rem this folder\'s home and config.json (test port ' + port + '), never the installed app\'s.',
    'rem   .\\cli version     .\\cli doctor     .\\cli status --watch',
    'setlocal',
    'set "HERE=%~dp0"',
    'set "BELLWETHER_AGENT_DIR=%HERE:~0,-1%"',
    'set "BELLWETHER_HOME=%HERE%home"',
    'set "BELLWETHER_CONFIG=%HERE%config.json"',
    'set "BELLWETHER_DATA_DIR="',
    'set "BELLWETHER_HOOK_TOKEN="',
    'if not exist "%BELLWETHER_HOME%" mkdir "%BELLWETHER_HOME%"',
    '"%HERE%bin\\bellwether.exe" %*',
    'exit /b %ERRORLEVEL%',
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
    'never installs hooks into your settings.',
    '',
    '  run-test.cmd                              the demo; quits after 30 minutes',
    '  run-test.cmd --quit-after 900             quits after 15 minutes instead',
    '  run-test.cmd --sessions                   real Claude Code sessions (below)',
    '  run-test.cmd --dump-desktop desktop.json  writes the monitors and windows to a file',
    '',
    'Real sessions: run-test.cmd --sessions makes a hook token (home\\hook-token, once) and',
    'hooks.json beside this file, then starts the strip and its brain. In another terminal,',
    'start Claude Code with: claude --settings <this folder>\\hooks.json',
    'A sheep walks on for it; /exit and it leaves. Your own settings.json is never changed.',
    '',
    'The command line: .\\cli <command> in this folder (cli.cmd runs bin\\bellwether.exe',
    'with this folder\'s home and port), e.g. .\\cli version, .\\cli doctor,',
    '.\\cli status --watch (not .\\bellwether: in this folder that is Bellwether.exe).',
    'hooks install and uninstall need --settings <a scratch copy>: this build refuses to',
    'edit your own settings.json.',
    '',
    'Quit early: the sheep icon in the notification area (it may be under the ^), Quit',
    'Bellwether. The logs are home\\shell.log (the strip) and home\\logs\\brain.log (the',
    'brain, with --sessions). Each run-test.cmd run starts a fresh shell.log',
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
  const args = ['build', '--release', '--locked', '--manifest-path', manifest, '--features', 'test-build,console-cli'];
  const [cmd, full] = o.xwin ? ['cargo', ['xwin', ...args, '--target', TRIPLE]] : ['cargo', args];
  process.stdout.write(`$ ${cmd} ${full.join(' ')}\n`);
  execFileSync(cmd, full, { stdio: 'inherit', cwd: AGENT });
  const dir = path.join(TARGET_DEV, ...(o.xwin ? [TRIPLE] : []), 'release');
  const exes = { app: path.join(dir, 'bellwether-shell.exe'), cli: path.join(dir, 'bellwether.exe') };
  for (const exe of Object.values(exes)) if (!fs.existsSync(exe)) fail(`${exe} is missing after the build`);
  return exes;
}

function assemble(exes, out) {
  fs.rmSync(out, { recursive: true, force: true });
  fs.mkdirSync(path.join(out, 'bin'), { recursive: true });
  fs.copyFileSync(exes.app, path.join(out, 'Bellwether.exe'));
  fs.copyFileSync(exes.cli, path.join(out, 'bin', 'bellwether.exe'));
  fs.writeFileSync(path.join(out, 'run-test.cmd'), runTestCmd());
  fs.writeFileSync(path.join(out, 'cli.cmd'), cliCmd());
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
  const exes = build(o);
  assemble(exes, o.out);
  const files = fs.readdirSync(o.out).sort();
  process.stdout.write(`files: ${files.join(', ')}\n`);
  process.stdout.write(`test build (unsigned, never ship one), port ${TEST_PORT}: ${o.out}\n`);
}
