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
 *   sharing-check.cmd    lists the programs that run only while you share your screen
 *                    (W5: the Teams check); reads the process list, changes nothing
 *   home\config.json   config.example.json on the test port (TEST_PORT): the folder's home
 *                    is where the app keeps its files, so the setup window can change it
 *   pets\<name>\animations.xml   Bellwether's own pets (checked against the pins compiled in)
 *   README.txt
 *
 * Since W7 the folder is laid out as an install is (`Bellwether.exe`, `bin\bellwether.exe`,
 * `pets\`), and the exes run in the app layout, as the installed app will (deviation W7-1):
 * the .cmd files set BELLWETHER_HOME to `home\` in it, and the exes find that home
 * themselves when started any other way (a Run value, Explorer, `bellwether` from PATH;
 * winstart.rs). The first start opens the setup window. The test build itself refuses what
 * would reach the live install (cli.rs `test_build_refusal`, deviation W2-2): `hooks
 * install|uninstall`, `token rotate` and `migrate` without --settings, and port 47417; its
 * setup window installs hooks into home\scratch-claude\settings.json (W7-3), and its start
 * at login (W6) uses Run values of its own.
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

/** Where the config goes in the folder: the home's config.json (W7-1). */
export const CONFIG_PATH = ['home', 'config.json'];

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
    'set "BELLWETHER_HOME=%HERE%home"',
    'set "BELLWETHER_AGENT_DIR="',
    'set "BELLWETHER_CONFIG="',
    'set "BELLWETHER_DATA_DIR="',
    'set "BELLWETHER_HOOK_TOKEN="',
    'set "LOGS=%BELLWETHER_HOME%\\logs"',
    'if not exist "%LOGS%" mkdir "%LOGS%"',
    'if /i "%~1"=="--dump-desktop" goto dump',
    'if /i "%~1"=="--sessions" goto sessions',
    'rem Each run starts a fresh log; the three before are kept (shell.previous.log is the last).',
    'call :rotate',
    'echo Starting the demo flock on every monitor (test port ' + port + ').',
    'echo Log: %LOGS%\\shell.log',
    'echo Quit: the sheep icon in the notification area (it may be under ^^), Quit Bellwether.',
    'start "" /wait "%HERE%Bellwether.exe" --demo --demo-monitors --heartbeat %* --quit-after 1800',
    'echo Stopped. Log: %LOGS%\\shell.log',
    'goto :eof',
    ':rotate',
    'rem shell.log -> shell.previous.log -> shell.previous-2.log -> shell.previous-3.log, so',
    'rem a run that hung and the restarts after it still leave its log.',
    'if exist "%LOGS%\\shell.previous-2.log" move /y "%LOGS%\\shell.previous-2.log" "%LOGS%\\shell.previous-3.log" >nul',
    'if exist "%LOGS%\\shell.previous.log" move /y "%LOGS%\\shell.previous.log" "%LOGS%\\shell.previous-2.log" >nul',
    'if exist "%LOGS%\\shell.log" move /y "%LOGS%\\shell.log" "%LOGS%\\shell.previous.log" >nul',
    'goto :eof',
    ':sessions',
    'rem The hook token (made once, kept in home\\hook-token), and the hooks as a file for',
    'rem claude --settings: your own settings.json is never touched.',
    'call "%HERE%cli.cmd" token init',
    'if errorlevel 1 exit /b 1',
    'call "%HERE%cli.cmd" hooks print --out "%HERE%hooks.json"',
    'if errorlevel 1 exit /b 1',
    'call :rotate',
    'echo Starting the strip and its brain for real sessions (test port ' + port + ').',
    'echo In another terminal (a Windows Terminal tab, or a herdr pane), start Claude Code with:',
    'echo   claude --settings "%HERE%hooks.json"',
    'echo Logs: %LOGS%\\shell.log and %LOGS%\\brain.log',
    'echo Quit: the sheep icon in the notification area (it may be under ^^), Quit Bellwether.',
    'start "" /wait "%HERE%Bellwether.exe"',
    'echo Stopped. Logs: %LOGS%\\shell.log and %LOGS%\\brain.log',
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
    'echo Could not write %~f2 (exit code %RC%). The cause is in the --dump-desktop lines of %LOGS%\\shell.log:',
    'findstr /c:"--dump-desktop:" "%LOGS%\\shell.log"',
    'exit /b %RC%',
  ];
  return lines.join('\r\n') + '\r\n';
}

/** cli.cmd: the console CLI with the folder's home and config, CRLF. */
export function cliCmd(port = TEST_PORT) {
  const lines = [
    '@echo off',
    'rem The Bellwether test build\'s command line (Windows plan W2): bin\\bellwether.exe with',
    'rem this folder\'s home and its config.json (test port ' + port + '), never the installed app\'s.',
    'rem   .\\cli version     .\\cli doctor     .\\cli status --watch',
    'setlocal',
    'set "HERE=%~dp0"',
    'set "BELLWETHER_HOME=%HERE%home"',
    'set "BELLWETHER_AGENT_DIR="',
    'set "BELLWETHER_CONFIG="',
    'set "BELLWETHER_DATA_DIR="',
    'set "BELLWETHER_HOOK_TOKEN="',
    'if not exist "%BELLWETHER_HOME%" mkdir "%BELLWETHER_HOME%"',
    '"%HERE%bin\\bellwether.exe" %*',
    'exit /b %ERRORLEVEL%',
  ];
  return lines.join('\r\n') + '\r\n';
}

/**
 * sharing-check.cmd (W5, WD5): which programs run only while a meeting app shares the
 * screen. It reads the process list three times (before, during, after sharing) and prints
 * the names seen only during, as `privacy.sharingProcesses.win32` spells them (with .exe).
 * Read-only; PowerShell with -NoProfile, nothing installed. CRLF; no `%` (cmd would expand it).
 */
export function sharingCheckCmd() {
  const ps = [
    "$names = { Get-Process | ForEach-Object { $_.ProcessName + '.exe' } | Sort-Object -Unique }",
    '$before = & $names',
    "Read-Host 'Not sharing yet. Start sharing your whole screen (Teams, Zoom...), wait 5 seconds, then press Enter' | Out-Null",
    '$during = & $names',
    "Read-Host 'Now stop sharing, wait 5 seconds, then press Enter' | Out-Null",
    '$after = & $names',
    '$only = $during | Where-Object { ($before -notcontains $_) -and ($after -notcontains $_) }',
    "if ($only) { 'Running only while sharing:'; $only | ForEach-Object { '  ' + $_ } } else { 'No program ran only while sharing.' }",
  ].join('; ');
  const lines = [
    '@echo off',
    'rem Which programs run only while you share your screen (Windows plan W5, the Teams check).',
    'rem It reads the list of running programs before, during and after a share, and prints the',
    'rem names seen only during it. It changes nothing.',
    `powershell -NoProfile -Command "${ps}"`,
    'pause',
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
    'The setup window (W7) opens the first time the strip starts (not for the demo), and',
    'again from the sheep icon: Set up Bellwether... It walks through start at login, hooks,',
    'the pet (Import... opens the Windows file dialog) and the bellwether command, and ends',
    'with doctor\'s checks. In this test build its hooks go into a scratch file,',
    'home\\scratch-claude\\settings.json, never your own; start Claude Code with',
    'claude --settings <this folder>\\home\\scratch-claude\\settings.json to use them.',
    'The bellwether command step adds this folder\'s bin to your own PATH (shown before it',
    'is changed; Remove takes it out). After it, a new terminal runs bellwether doctor from',
    'anywhere, with this folder\'s home. The config is home\\config.json (the test port).',
    '',
    'The command line: .\\cli <command> in this folder (cli.cmd runs bin\\bellwether.exe',
    'with this folder\'s home and port), e.g. .\\cli version, .\\cli doctor,',
    '.\\cli status --watch (not .\\bellwether: in this folder that is Bellwether.exe).',
    'hooks install and uninstall need --settings <a scratch copy>: this build refuses to',
    'edit your own settings.json.',
    '',
    'Quit early: the sheep icon in the notification area (it may be under the ^), Quit',
    'Bellwether. The logs are home\\logs\\shell.log (the strip) and home\\logs\\brain.log',
    '(the brain). Each run-test.cmd run starts a fresh shell.log (the three runs before',
    'are kept: home\\logs\\shell.previous.log, -2 and -3), so attach shell.log after the run',
    'you are reporting on. --dump-desktop adds its outcome to the current shell.log, and',
    'exits with a non-zero code if the file could not be written.',
    '',
    'Start at login (W6): the setup window\'s first step, or .\\cli token init (once), then',
    '.\\cli autostart install --write.',
    'At each sign-in Windows starts the strip and its brain from this folder, with its own',
    'home and port: real sessions all day, no .cmd needed. They are under their own names,',
    '"Bellwether (test)" and "Bellwether Brain (test)", never an installed Bellwether\'s.',
    '  .\\cli autostart status          whether they are on, or turned off in Task Manager',
    '  .\\cli autostart settings        opens Settings > Apps > Startup',
    '  .\\cli shell stop | start | restart    the strip; the brain keeps running',
    '  .\\cli autostart uninstall --write     removes both and stops them',
    'Run autostart uninstall --write before you delete or move this folder. While the strip',
    'runs, it starts the brain again within about 15 s if the brain stops.',
    '',
    'sharing-check.cmd: start it, then share your screen in a meeting app when it asks, and',
    'stop when it asks again. It prints the programs that ran only while you shared (W5).',
    '',
    'Windows may warn that the app is unrecognised (it is unsigned): More info, Run anyway.',
    '',
    'Cleaning up: .\\cli autostart uninstall --write; Remove from PATH in the setup window',
    '(its bellwether command step) if you added it; then delete this folder, and',
    '%LOCALAPPDATA%\\local.bellwether.p' + port + ' (WebView2\'s files for this test build).',
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
  fs.writeFileSync(path.join(out, 'sharing-check.cmd'), sharingCheckCmd());
  fs.mkdirSync(path.join(out, 'home'), { recursive: true });
  fs.writeFileSync(path.join(out, ...CONFIG_PATH), testConfig(fs.readFileSync(path.join(AGENT, 'config.example.json'), 'utf8')));
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
