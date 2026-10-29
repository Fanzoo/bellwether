# Bellwether: Windows plan

A plan for a Windows version of Bellwether. The sheep walk along the top of the taskbar,
one per Claude Code session, as they do along the Dock on macOS. Windows gets the same
app, brain, CLI and setup window, signed and updating itself.

Status: proposal, with the maintainer's decisions of 2026-10-20 applied (see "Decisions
(maintainer, 2026-10-20)" under Open questions). W1 is built and has had a first run on
the maintainer's PC; W2's first part is built; W3 is built and awaits its PC checks
(their status sections); nothing else is built yet. Items marked
**ASSUMPTION** are technical guesses that a milestone proves. Bracketed numbers such as
[4] point to the Sources list at the end. Crate versions were read from `Cargo.lock` and
crates.io on 2026-10-19 [21]. The Tauri, tao and wry behaviour quoted here was read from
the sources of the versions in `Cargo.lock` [20].

This plan does not repeat the macOS plan. [DISTRIBUTION-PLAN.md](DISTRIBUTION-PLAN.md)
(D1–D10, M1–M13 and its deviations) is the reference for everything that is the same on
both systems: the brain's contracts, the hook guarantees, the conformance suite, the
setup window's commands and capabilities, the updater's policy, the release environment's
rules, and the drag (D10, M12). Milestones here are numbered **W1–W10**, so they never
clash with M1–M13.

---

## Goal

Today Bellwether runs only on a Mac. After this plan ships, someone on Windows 11 can
download `Bellwether_<ver>_x64-setup.exe` (or run `winget install Fanzoo.Bellwether`),
install it without admin rights, and get the same thing a Mac user gets:

- a setup window that turns on start at login, installs the hooks into
  `%USERPROFILE%\.claude\settings.json` with the same preview-then-confirm, and offers
  the `bellwether` command;
- sheep standing on the taskbar's top edge of whichever monitor holds their terminal;
- a click on a sheep that brings its terminal window forward;
- the strip hidden while a known screen-sharing app runs, and while a full-screen app
  covers its monitor, with the sheep still in his own screenshots (capture exclusion is
  an opt-in, WD5);
- signed updates from GitHub Releases.

macOS is unchanged by this work. Both platforms ship from one tag, one version and one
`latest.json`.

## Non-goals

- **Changing macOS behaviour.** Any change to shared code must keep the macOS tests and
  conformance suite as they are. Where Windows needs a different rule, it goes behind a
  per-OS seam, never as a changed macOS default.
- **Linux.** It stays possible (WD13), but it gets its own plan later.
- **Claude Code inside WSL.** A session running in WSL has its own `~/.claude` inside
  Linux, and in WSL2's default NAT networking its `127.0.0.1` is not Windows'. Bellwether
  for Windows serves sessions that run on Windows itself (Q11).
- **Selecting the exact Windows Terminal tab.** No public interface maps a session to a
  tab (WD4). A click brings the right window forward, not the tab. Inside herdr, herdr's
  own workspace, tab and pane *are* selected, through herdr's CLI, as on macOS (WD4).
- **Standing on the taskbar's icons only.** macOS confines sheep to the Dock's width with
  Accessibility. The Windows taskbar spans the monitor, so sheep use the full width.
  Confining them to the centred icons of Windows 11 is not planned.
- **Microsoft Store and MSIX.** Tauri builds NSIS and MSI, not MSIX [3]. The Store can
  come later (Q5).
- **Scoop, Chocolatey.** Not at first (Q5).
- **ARM64 builds.** x64 only at first. Windows on ARM runs x64 apps under emulation (Q3).
- **Windows 10 as a tested target.** It should work, and nothing is done to break it,
  but by-hand checks run on Windows 11 (Q2).
- **New features.** No new pet behaviours, states, bubble kinds or config keys beyond
  what Windows needs.

## Surface

The same one executable and three modes as macOS (DISTRIBUTION-PLAN.md, Surface), plus a
second, console executable for the CLI:

| Invocation | Mode | What it does |
|---|---|---|
| Start menu, Run key at login, `Bellwether.exe` with no subcommand | **app** | the strips, the tray icon, the setup window on first run |
| `Bellwether.exe serve` (Run key at login, or the app starting it) | **brain** | the headless listener on `127.0.0.1:47417`; never starts Tauri or WebView2 |
| `bellwether <subcommand>` in a terminal (`<install>\bin\bellwether.exe`, on the user's PATH) | **CLI** | the same commands, flags, output and exit codes as on macOS |

Why a second executable on Windows (WD7): `Bellwether.exe` is a GUI-subsystem program
(`main.rs` already sets `windows_subsystem = "windows"` for release builds). Its output
never reaches the terminal that started it, and the prompt returns before it finishes.
The CLI needs a console-subsystem program.

Commands that change meaning on Windows:

- `autostart install|uninstall|status|settings`: Run-key entries instead of SMAppService
  (WD6). `settings` opens Settings > Apps > Startup.
- `permissions …`: nothing to ask for on Windows. It prints that no permission is needed
  and exits 0.
- `migrate`, the legacy LaunchAgent checks and `--to-app`: macOS only. On Windows they
  say "nothing to migrate" and exit 0.
- `shell start|stop|restart`: work as on macOS, through the single-instance plugin and a
  tokened stop (WD6).

Files go under `%APPDATA%\Bellwether\` and logs under `%LOCALAPPDATA%\Bellwether\logs\`
(WD8).

## Code, not a model

Everything here is deterministic code: Rust in `crates/` and `shell/src-tauri`,
JavaScript in `shell/src/player`, Node scripts in `scripts/`, NSIS hooks and GitHub
workflows. No model is in the loop at runtime.

## Stack

| Piece | Choice | Why |
|---|---|---|
| Language, toolchain | Rust 1.99.0 (installed here; release.yml pins it), edition 2021, target `x86_64-pc-windows-msvc` (installed here) | the same toolchain as macOS |
| App framework | `tauri` =2.12.1, `tauri-build` =2.7.1, `@tauri-apps/cli` 2.12.1 (as pinned; 2.12.4 is current) | the same pins as macOS. Bumps are a separate PR, for both platforms |
| Window system | `tao` 0.37.1 and `wry` 0.57.0 (through Tauri), WebView2 | tao already does click-through, no-activate, skip-taskbar and (opt-in) capture exclusion on Windows [20] |
| Win32 calls | `windows-sys` 0.61.2 (already in `Cargo.lock`), as a `cfg(windows)` dependency | the taskbar, window list, foreground, process list, registry, ACLs and console calls. No new crate for these |
| Process environment and command line | own code over `windows-sys` (`NtQueryInformationProcess` + `ReadProcessMemory`) | the locator's marker match (WD4). Fallback: `sysinfo` 0.39.6 (**new**), if the own code proves fragile |
| File dialog, message boxes | `rfd` 0.17.2 (**new**, `cfg(windows)` only), called from Rust | the pet Import… dialog and the update prompt. Rust only: no plugin, so no webview command (as `NSOpenPanel` and `NSAlert` on macOS) |
| Path display | `dunce` 1.0.5 (already in `Cargo.lock`) | `canonicalize` returns `\\?\C:\…` on Windows; keys and messages need `C:\…` |
| Socket option | `socket2` 0.6.5 (already in `Cargo.lock`) | `SO_EXCLUSIVEADDRUSE` on the brain's listener (WD6) |
| Single instance | `tauri-plugin-single-instance` =2.5.2 (already used) | a named mutex plus `WM_COPYDATA` on Windows [20] |
| Updates | `tauri-plugin-updater` =2.13.2 (already used; 2.14.0 is current) | reuses the signed NSIS installer as the update [1] |
| Installer | Tauri's NSIS bundler, `installMode: currentUser` | no admin, installs under `%LOCALAPPDATA%` [3]; MSI needs admin and WiX on Windows |
| Signing | Azure Artifact Signing (formerly Trusted Signing) through `artifact-signing-cli` as Tauri's `signCommand` [2][8], in Fanzoo's existing Azure account (decided 2026-10-20) | about USD 9.99 a month, no hardware token. See WD9 |
| CI | GitHub Actions `windows-2025` runners (pin the image, as release.yml pins `macos-15`) | build, test, bundle, sign |
| Dev host | the maintainer's Mac, through agents; no Windows machine | so CI and pure functions carry most of the testing (WD12) |

New crates go through `scripts/third-party.mjs --check` as before.

---

## Every macOS-specific piece, and its Windows counterpart

From `grep -rn 'target_os\|cfg(unix)\|cfg!(' crates shell/src-tauri/src` and a read of the
modules. "W" is the milestone that builds the Windows side.

| Piece (file) | macOS today | Windows counterpart | W |
|---|---|---|---|
| Dock prefs (`dock.rs`) | CFPreferences `com.apple.dock` autohide and side | none needed: the monitor's work area already excludes a shown taskbar (WD1). A pure `taskbar_edge(screen, work)` for logs and doctor | W1 |
| Dock frame (`ax.rs`) | Accessibility, the Dock's exact extent | not built (Non-goals). `trusted()` stays false | — |
| Strip window (`strips.rs`) | WebKit occlusion SPI, `visible_on_all_workspaces`, logical frame | `WS_EX_TOOLWINDOW` added, physical frame per monitor, WebView2 throttling checked (WD2, WD3) | W1 |
| Click-through (`clickthrough.rs`) | `set_ignore_cursor_events`; cursor in global points | the same call; cursor and monitors in physical pixels (WD3) | W1, W4 |
| Mouse button state (`clickthrough::mouse_down`) | `CGEventSourceButtonState` | `GetAsyncKeyState(VK_LBUTTON / VK_RBUTTON / VK_MBUTTON)`, no permission | W4 |
| Window list (`locator.rs` `cg`) | `CGWindowListCopyWindowInfo` | `EnumWindows` + `GetWindowThreadProcessId` + `DwmGetWindowAttribute(DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_CLOAKED)` | W3 |
| Process list (`locator.rs`, `ps`) | `/bin/ps -Eww` | Toolhelp snapshot (pid, parent, exe name) plus the environment read for marker match (WD4) | W3 |
| Focus (`locator.rs`) | `open -a`, AppleScript to iTerm2 and Terminal | `SetForegroundWindow` on the terminal's window, `ShowWindow(SW_RESTORE)` if minimised (WD4) | W3 |
| Automation probe (`onboarding.rs` `mac_ae`) | `AEDeterminePermissionToAutomateTarget` | none: no permission on Windows | W7 |
| Screen-share hide (`privacy.rs`) | `sharingType none` (on by default), libproc process names | Toolhelp process names as the main layer; `WDA_EXCLUDEFROMCAPTURE` (tao's `set_content_protection`) only when opted in, default off on Windows (WD5) | W5 |
| Full screen (`privacy.rs` comment) | macOS keeps the strip off full-screen Spaces | a foreground-covers-monitor detector, honouring the existing `privacy.hideWhenFullscreen` (WD5) | W5 |
| Login items (`login_items.rs`) | SMAppService agent and main app | HKCU `Run` values, read with `StartupApproved\Run` (WD6) | W6 |
| Brain agent restart (`agent.rs`) | `launchctl kickstart -k` | a tokened stop, then a detached `Bellwether.exe serve` (WD6) | W6 |
| Child brain stop (`brain.rs` `stop_child`) | SIGTERM, then SIGKILL after 2 s | the tokened stop, then `TerminateProcess` after 2 s | W2 |
| Parent watchdog (`crates/brain/src/lib.rs`) | `getppid()` every 250 ms | `OpenProcess(SYNCHRONIZE)` on `BELLWETHER_PARENT_PID` and a wait | W2 |
| Signals (`signals.rs`, brain `stop_signal`) | SIGINT, SIGTERM, SIGHUP | Ctrl-C in the console CLI only; the brain stops through `/control` (WD6) | W2 |
| Identity (`identity.rs`) | bundle path, `codesign -dv`, `proc_pidpath`, launchd session | the install directory, `WinVerifyTrust` for doctor, `QueryFullProcessImageNameW`; no session check | W2, W8 |
| CLI hand-off (`actions.rs`, `cli.rs` autostart re-exec) | `open -b … --args --action` so TCC names the app | not needed: no TCC. The CLI writes the Run keys itself | W6 |
| CLI on PATH (onboarding `cli_tool_install`) | `~/.local/bin` symlink, Homebrew `binary` | `<install>\bin` added to the user's PATH (WD7) | W7 |
| Pet import dialog (`onboarding.rs`) | `NSOpenPanel` | `rfd::FileDialog`, from Rust | W7 |
| Update prompt (`updater.rs`) | `NSAlert` | `rfd::MessageDialog`, from Rust | W8 |
| Activation policy (`lib.rs`) | `Accessory` (no Dock icon) | nothing: strips are tool windows, the tray is the only presence | W1 |
| Paths (`home.rs`, `outfile.rs`, `lib.rs`, `tray.rs`, `cli.rs`, `migration.rs`, `agent.rs`, `settings.rs`) | `$HOME` | `%USERPROFILE%` through one `user_home()` in core; `%APPDATA%` (WD8) | W2 |
| File modes (`persist.rs`, `hooks.rs`, `token.rs`, `migrate.rs`, `toapp.rs`) | 0600 files, 0700 directories | a protected DACL for the current user (WD8) | W2 |
| Port owner (`doctor.rs`) | `lsof`, `ps` | `GetExtendedTcpTable` + `QueryFullProcessImageNameW` (read only) | W2 |
| Local time (`doctor.rs`) | `localtime_r` | `GetLocalTime` / `SystemTimeToTzSpecificLocalTime` | W2 |
| Managed settings (`doctor.rs`) | `/Library/Application Support/ClaudeCode/` | **today's Windows path is wrong**: Claude Code reads `C:\Program Files\ClaudeCode\managed-settings.json` and no longer `C:\ProgramData\…` [10]; also `HKLM` and `HKCU\SOFTWARE\Policies\ClaudeCode` value `Settings` | W2 |
| Signing (`build-app.mjs`, release.yml) | Developer ID, notarytool, stapler | Authenticode through Artifact Signing (WD9) | W9 |
| Distribution (tap.yml, `packaging/homebrew`) | Homebrew cask | winget manifest (WD9) | W10 |
| Legacy (`legacy.rs`, `migrate.rs`, `migration.rs`, `toapp.rs`) | repository LaunchAgents | none: no earlier Windows installs exist. Compiled out of the Windows paths | W2 |

---

## Design decisions

Each one: the recommendation, the alternatives, and why.

### WD1. Where the pets walk: the taskbar's top edge

**Recommendation: the strip sits on the bottom edge of each monitor's work area, always.**

- The work area (`GetMonitorInfo` `rcWork`, which tao exposes as `Monitor::work_area()`)
  already excludes a shown taskbar. With the taskbar at the bottom, the work area's bottom
  edge *is* the taskbar's top edge, so the sheep stand on the taskbar. This is the same
  rule `strip_rect_in` uses for a shown Dock today.
- **Auto-hide:** the work area is the whole monitor, so the strip sits on the screen's
  bottom edge, as with an auto-hidden Dock. When the taskbar slides up it covers the
  sheep's feet for a moment, as the Dock does.
- **Taskbar on the top, left or right** (Windows 10; Windows 11 Insider builds since
  mid-2026 [14]): the work area's bottom is the screen's bottom, so the sheep walk along
  the bottom of the screen, clear of a side taskbar. Same as a side Dock on macOS.
- **Several monitors:** each monitor has its own work area. Secondary taskbars ("Show my
  taskbar on all displays") reserve space on their own monitor (**ASSUMPTION**, checked by
  hand in W1). A monitor without a taskbar gets the strip on its screen bottom.
- **Windows 10 and 11 alike:** nothing here differs by version.
- **Following changes:** the strip already re-reads its frame every 2 s (`follow_dock` in
  `lib.rs`). On Windows the same loop re-reads the monitors' work areas, so a taskbar
  that moves, auto-hides or changes size is followed within 2 s, with no taskbar API.
- **Logs and doctor:** a pure function `taskbar_edge(screen, work) -> Bottom | Top | Left
  | Right | None` says which edge the work area is inset from. That is logged, and
  doctor shows it. It is testable on the Mac.

Alternatives:

- **`SHAppBarMessage(ABM_GETTASKBARPOS)`** [5]: gives the primary taskbar's rectangle and
  edge only. Secondary taskbars need `FindWindow("Shell_SecondaryTrayWnd")`, an
  undocumented class name. Auto-hide per monitor needs `ABM_GETAUTOHIDEBAREX`, which
  Chromium found unreliable with a monitor rectangle [19]. More code, less certain, and
  it adds nothing the work area doesn't already give.
- **Standing on the taskbar's icons** (UI Automation on the task list): the analogue of
  the Accessibility Dock extent. Rejected for now (Non-goals): the taskbar spans the whole
  monitor, so the full width is already "on the taskbar".
- **Registering as an app bar** (`ABM_NEW`) to get `ABN_FULLSCREENAPP` and position
  notices: it would make Bellwether part of the shell's layout. Rejected: risk of
  reserving screen space by mistake, for a notice the poll already covers.

### WD2. The overlay window

**Recommendation: the same `WebviewWindowBuilder` as macOS, plus three Windows-only
steps.** What tao 0.37.1 already does on Windows, read from its source [20]:

- `transparent(true)`: WebView2's default background colour is set to transparent by wry.
- `set_ignore_cursor_events(true)`: adds `WS_EX_TRANSPARENT | WS_EX_LAYERED`. Clicks fall
  through to the window underneath, in any process. The 30 Hz cursor poll turns it off
  over a sheep, as on macOS.
- `focusable(false)`: adds `WS_EX_NOACTIVATE`. A click on a sheep doesn't take focus from
  the terminal.
- `focused(false)` at creation: tao shows the window with `SW_SHOWNOACTIVATE`.
- `skip_taskbar(true)`: removes the taskbar button through `ITaskbarList::DeleteTab`.
- `always_on_top(true)`: `HWND_TOPMOST`.
- `set_content_protected(true)`: `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`. Off by
  default on Windows (WD5); called only when `privacy.captureExclusion` is true.
- Per-monitor DPI awareness v2 is set for the process.

The three Windows-only steps:

1. **`WS_EX_TOOLWINDOW` on each strip**, through the window's HWND after it is built.
   `DeleteTab` hides the taskbar button but not the Alt-Tab entry; a tool window is in
   neither.
2. **WebView2 throttling.** `background_throttling(Disabled)` is unsupported on Windows
   (wry 0.57.0's own docs) [20]. The strip is topmost, so it is rarely covered, but
   WebView2 can still treat a mostly transparent, layered window as hidden. W1 measures
   it (the page's heartbeat in the log over 10 minutes, with Win+D, a locked screen, and
   another window maximised). If the page is throttled, the fallback is
   `additional_browser_args` with `--disable-features=CalculateNativeWinOcclusion` and
   `--disable-background-timer-throttling`. That replaces wry's default arguments, so the
   list must keep them (`--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`
   merged into the one `--disable-features`). Every webview in the process (strips and the
   setup window) must get the same arguments, or they need separate data directories
   (Tauri's own warning) [20].
3. **Click-through toggling cost.** tao adds and removes `WS_EX_LAYERED` together with
   `WS_EX_TRANSPARENT`. Toggling `WS_EX_LAYERED` on a WebView2 window may flash or repaint
   the strip, and Electron found it interacts with capture exclusion (a black window after
   hide and show) [18]; that matters only to users who opt in to exclusion. W1 watches for a flash while the cursor crosses sheep. If there is
   one, the fallback is to keep `WS_EX_LAYERED` on for good and toggle only
   `WS_EX_TRANSPARENT` with `SetWindowLongPtrW`.

Other behaviour:

- **Virtual desktops:** tao's `visible_on_all_workspaces` does nothing on Windows (it has
  no Windows implementation) [20]. W1 records whether a topmost tool window follows a
  desktop switch. If it doesn't, the fallback is to move the strips to the current desktop
  on `IVirtualDesktopManager` changes. **ASSUMPTION:** tool windows show on every desktop.
- **Full screen:** a topmost window shows over a borderless full-screen app (a video in
  F11, a game in borderless mode). The detector is WD5.
- **The setup window** is a normal window, in the taskbar while open.

Alternatives: a native layered window drawn with Direct2D instead of WebView2. Rejected:
it would fork the player, which stays JavaScript on every platform (DISTRIBUTION-PLAN.md,
Non-goals).

### WD3. Coordinates, DPI and monitors

**Recommendation: on Windows, physical pixels are the shared space for the cursor,
monitors, strips and terminal windows. Logical pixels are only used inside one strip.**

The problem. On macOS every position is in one global space of points. The code divides
the cursor by the primary monitor's scale and each monitor by its own scale
(`cursor_points`, `window_points`, `monitors_of`). On Windows with per-monitor DPI v2,
`GetCursorPos`, monitor rectangles and window rectangles are all physical pixels of one
virtual screen. Dividing each by a different scale breaks the layout. Example: a primary
1920×1080 at 100% at x 0, and a 3840×2160 at 150% to its right at physical x 1920. Divided
by its own scale, the second monitor starts at x 1280, inside the first one. The cursor
maps to the wrong strip.

What changes:

- **A `DesktopSpace` seam** with two implementations: `Points` (macOS, today's maths,
  unchanged) and `Physical` (Windows). `locate_cursor`, `monitor_for`, the cursor poll and
  the drag mapping take positions in the seam's space. The existing macOS tests stay as
  they are; Windows gets its own table of cases (side by side, stacked, a monitor left
  of the primary at negative x, mixed 100%/150%/175%).
- **Strip frames** are computed per monitor in physical pixels (the work area's bottom
  minus `strip.heightPx × scale`) and set with `PhysicalPosition` and `PhysicalSize`.
  `LogicalPosition` would be converted with the window's *current* scale, which is the
  wrong one while a new strip is still on the monitor it was created on.
- **DPI changes:** when a strip gets `ScaleFactorChanged` (a scale changed in Settings, or
  the window moved monitors), Windows proposes a new size. The shell puts the strip back
  to its computed physical frame at once.
- **Inside a strip,** the player keeps working in CSS pixels. The shell sends the strip's
  own scale with its boxes, as it does now.
- **Monitor names:** tao names Windows monitors `\\.\DISPLAY1` and so on. They can change
  when a monitor is plugged in again. Drops and slots keyed by monitor already end when
  the display set changes (M12), so this is safe.

Alternatives: make the process DPI-unaware or system-aware. Rejected: Windows would
bitmap-stretch the strip on high-DPI monitors (blurry sheep), and positions would be
virtualised.

### WD4. Clicking a sheep: finding and raising its terminal

**Recommendation: find the session's `claude` process, walk up its parents to the first
process that owns a visible top-level window, use that window to place the sheep, and
bring it forward with `SetForegroundWindow` on a click.** Window level, not tab level.
**Inside herdr** (the maintainer's setup: herdr in Windows Terminal), the walk starts from
herdr's attached client instead, and herdr's CLI selects the pane (below).

How the locator's existing chain maps:

- **`herdr`** (first in the Windows chain; the maintainer runs Claude Code in herdr
  inside Windows Terminal). See "herdr on Windows" below.
- **`process-tree`** (the main strategy outside herdr):
  - The process list from a Toolhelp snapshot: pid, parent pid, exe name. No permission
    is needed for the current user's processes.
  - The `claude` process is `claude.exe` (native install) or `node.exe` running Claude
    Code's `cli.js` (npm install). Recognising the second needs the command line.
  - **Marker match:** as on macOS, a session's markers (`WT_SESSION`, `TMUX_PANE`, …)
    must all be in that process's environment. Windows has no `ps -E`; the environment
    is read from the process's PEB (`NtQueryInformationProcess` +
    `ReadProcessMemory`), for processes of the same user only. Only the marker variables
    are kept; nothing else is logged or stored, as on macOS. `WT_SESSION` is unique per
    Windows Terminal pane, so in Windows Terminal this is exact.
  - **The window owner:** `EnumWindows`, visible, not cloaked (`DWMWA_CLOAKED`, so windows
    on another virtual desktop or suspended UWP windows don't count), not a tool window.
    Bounds from `DWMWA_EXTENDED_FRAME_BOUNDS` (physical, without the invisible resize
    border). Window titles are not read.
  - Expected parents (**ASSUMPTION**, recorded on the maintainer's PC in W3): Windows
    Terminal starts shells itself through ConPTY, so the chain is `claude.exe` → `pwsh.exe`
    → `WindowsTerminal.exe`. VS Code: `claude.exe` → shell → VS Code's pty host → `Code.exe`.
  - One `WindowsTerminal.exe` owns all its windows, so with several Windows Terminal
    windows the result is `app` confidence, as for VS Code on macOS.
- **`vscode`:** the same, `TERM_PROGRAM=vscode` (VS Code sets it on Windows too).
  Secondary: the maintainer doesn't use VS Code, so it is covered by fixtures and unit
  tests, and checked by hand only if VS Code is at hand.
- **`app-activate`:** a table from `TERM_PROGRAM` or the marker (`WT_SESSION` →
  `WindowsTerminal.exe`, `vscode` → `Code.exe`, `WezTerm` → `wezterm-gui.exe`, …) to an exe
  name; the most recently active visible window of that exe is brought forward.
- **`iterm2`, `terminal-app`:** not built on Windows. The chain already skips a strategy
  "not built on this OS" with one log line.
- **Defterm hand-off:** when Windows Terminal is the default terminal and a shell is
  started from a shortcut, the shell's parent is not Windows Terminal. The walk then finds
  no window owner, and `app-activate` takes over (it has `WT_SESSION`).

**herdr on Windows.** What herdr is and does, from its docs [22][23] (herdr 0.9.5 docs):

- herdr is a terminal multiplexer for coding agents. A background **server** owns the
  panes and their processes, and keeps them running when the client detaches or its
  window closes. A **client** (`herdr`) attaches from Windows Terminal or the PowerShell
  app. Native Windows support is generally available, on ConPTY.
- Every pane gets `HERDR_PANE_ID`, `HERDR_TAB_ID`, `HERDR_WORKSPACE_ID`, `HERDR_SOCKET_PATH`
  and `HERDR_ENV=1`. A running process keeps the environment it was started with.
- The CLI and the socket API are one surface. On Windows the socket is a named pipe (its
  name isn't documented), and herdr's docs recommend the CLI over raw pipe clients for
  portable Windows use [23]. `pane.get`, `workspace.focus`, `tab.focus` and `agent.focus`
  exist [23]. `pane focus` and `agent focus` also mark the pane as seen [22].
- herdr installs under `%USERPROFILE%\.herdr\…`, puts its active release on PATH, and keeps
  `%LOCALAPPDATA%\Programs\Herdr\bin` as an alias [22].

What that means for the locator:

- **The process walk from `claude` does not reach Windows Terminal.** `claude.exe` is a
  child of herdr's *server*, which is not under Windows Terminal (it outlives the client,
  so it can't be). Walking up from `claude` finds no window. This is the same on macOS,
  which is why the macOS `herdr` strategy starts from the attached client
  (`herdr_host`).
- **So on Windows, as on macOS, the walk starts from the `herdr.exe` client:** the herdr
  processes of the same user whose parent chain reaches a window owner
  (`herdr.exe` → `pwsh.exe` → `WindowsTerminal.exe`). The server is told apart from a
  client by its parent chain and its arguments (**ASSUMPTION**: the server's own
  `herdr.exe` either has no windowed ancestor or a `server` argument; the W3 dump records
  the herdr processes' parents and their subcommand word only, to settle it). macOS uses
  "has a tty"; Windows has no tty, so this rule replaces it.
- **`WT_SESSION` is not trusted for a session inside herdr.** `claude.exe` inherits the
  server's environment, which holds the `WT_SESSION` of whichever tab first started the
  server, or none. When the hook carries `HERDR_PANE_ID`, the `WT_SESSION` header is
  ignored for matching, and `process-tree` never runs for that session (`herdr` answers
  first, or `app-activate` does). Outside herdr, `WT_SESSION` matching stays as above.
- **Which Windows Terminal window:** with one attached client, exact. With several
  attached clients (several Windows Terminal windows or tabs each running `herdr`), the
  most recently active host window wins, at `app` confidence, as on macOS.
- **Focus:** `herdr workspace focus <ws>`, `tab focus <tab>`, `agent focus <pane>` (the same
  commands the macOS strategy runs), then the host Windows Terminal window comes forward
  with `SetForegroundWindow`. So **the right herdr pane is in scope**: herdr offers the CLI
  for it. What stays out of scope is the **Windows Terminal tab** that hosts the herdr
  client: if that client runs in a background WT tab, the WT window comes forward on
  whatever tab it shows (no API selects a WT tab; see Tabs below).
- **`HERDR_SOCKET_PATH` on Windows is a pipe name, not a file.** The macOS
  `check_socket` (a real socket file owned by the user) doesn't apply. The Windows rule:
  the value must look like `\\.\pipe\…`, and the pipe's server
  (`GetNamedPipeServerProcessId`) must be a `herdr.exe` of the same user; otherwise the
  socket is refused with one log line, as on macOS. If the value turns out not to be a
  pipe path (W3 records it, with the user name masked), the rule follows what herdr sets.
- **Where herdr is:** `locator.herdrPath` (default `herdr`, resolved on PATH), then
  `%LOCALAPPDATA%\Programs\Herdr\bin\herdr.exe`, the documented alias. The same 4 s timeout
  and 5-minute back-off as macOS.
- **Header values** keep the macOS checks before they reach herdr (`valid_pane_id`, never
  starting with `-`).

**Raising the window:** `ShowWindow(SW_RESTORE)` if it is minimised, then
`SetForegroundWindow`. Windows only lets a process take the foreground under conditions
[7]. One of them is that the process "received the last input event". The click on the
sheep is that input: the strip accepted the mouse for it. **ASSUMPTION**, proved in W3: a
click on a `WS_EX_NOACTIVATE` window counts. If it doesn't, Windows flashes the
terminal's taskbar button instead, and the log says so. No Alt-key injection or
`AttachThreadInput` tricks: they work around a user protection.

**Tabs:** not feasible reliably. `wt.exe -w <window> focus-tab -t <index>` exists [12], but
nothing maps a `WT_SESSION` to a window id or tab index. UI Automation on the tab strip
could match by title, but titles are set by the shell and by Claude Code and are not
stable. Recorded as later work.

Alternatives: UI Automation for everything (heavy COM, slow on big trees); `sysinfo` for
the process list and environment (a good fallback, one crate; chosen only if the own code
is fragile).

### WD5. Screen sharing and full screen

**Decision (maintainer, 2026-10-20): the sheep must appear in his own screenshots.** So on
Windows capture exclusion is **off by default**, and screen-sharing hiding relies on the
process list, as on macOS, plus the full-screen detector. Exclusion stays available as an
opt-in.

- **The process list (main layer).** `privacy.sharingProcesses.win32` already exists, with
  `CptHost.exe` (Zoom's sharing process). On Windows the names come from a Toolhelp
  snapshot every 500 ms, with the same 2 s debounce, honouring `privacy.hideWhenSharing`.
  Windows names keep `.exe` and are not cut at 31 bytes; `sharing_warnings_for` gets
  Windows rules (a name without `.exe` is a WARN). W5 records which process, if any,
  runs only while Teams (the new Teams, `ms-teams.exe`) shares, and adds it to the
  default list only if one exists. As on macOS, a meeting in a browser (Meet, Teams on the
  web) has no such process: the README says to use *Hide everything (for screen sharing)*
  first.
- **Full screen.** `privacy.hideWhenFullscreen` (default true, unused on macOS) gets a
  Windows detector: every 500 ms, the foreground window's bounds are compared with each
  monitor's full rectangle. A window that covers a monitor (and is not the desktop's
  `Progman` or `WorkerW` window) hides that monitor's strip only, with the 2 s debounce.
  `SHQueryUserNotificationState` adds the cases a rectangle can miss: an exclusive D3D
  full-screen app and presentation mode hide every strip. This also covers sharing a
  single full-screen window or a slideshow.
- **Capture exclusion (opt-in).** The existing key `privacy.captureExclusion` is kept, with
  a per-OS default: **false on Windows**, true on macOS (unchanged; it is what macOS has
  shipped). No new key: the name already matches its siblings (`hideWhenSharing`,
  `hideWhenFullscreen`, `sharingProcesses`), and a second key for the same switch would
  only confuse. `config.example.json` and the README say the default differs by OS, and
  doctor prints the effective value.
  - When true, the strips get `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)` through
    `set_content_protected(true)` [4][20]: shown on the monitor, absent from captures
    (Zoom, Teams, Meet in a browser, OBS, the Snipping Tool, and his own screenshots). It
    needs Windows 10 2004 or later and DWM composition [4]. Microsoft says it is not a
    security guarantee [4].
  - When true, the log and doctor read `GetWindowDisplayAffinity` back for each strip.
  - The shell reads it at start, like every `privacy.*` key: `bellwether shell restart`
    after a change.

Alternatives:

- **Exclusion on by default** (the plan before 2026-10-20): the strongest layer, but it
  hides the sheep from the maintainer's own screenshots. Rejected by the maintainer.
- **Hiding on any capture-API use:** not observable from outside the capturing process.
- **A per-OS key name** (`captureExclusionWin32`): rejected; one key with a per-OS default
  is simpler, and `sharingProcesses` already shows that per-OS differences live in values,
  not names.

### WD6. Processes: the brain, start at login, single instance, updates

**Recommendation: two HKCU `Run` values, the app supervising the brain, and a tokened
stop instead of signals.**

Start at login:

- `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`:
  - `Bellwether` = `"<install>\Bellwether.exe"` (the strip);
  - `Bellwether Brain` = `"<install>\Bellwether.exe" serve`.
- No admin. Both show in Task Manager's Startup apps and in Settings > Apps > Startup,
  where the user can turn them off.
- **Status:** a value turned off there is kept in
  `HKCU\…\Explorer\StartupApproved\Run`. Its first byte is `02` when on and `03` when off.
  That layout is from community testing, not Microsoft documentation [17], so the reader
  treats anything it doesn't recognise as `unknown`, never as off. "Turned off in Task
  Manager" is the Windows analogue of `requiresApproval`: `autostart status`, doctor and
  the setup window say so and offer `ms-settings:startupapps`.
- `autostart install --write` rewrites the paths if the install moved, so an update (same
  path) changes nothing.

The brain:

- **Crash restart.** launchd's `KeepAlive` has no Run-key equivalent. While the app runs,
  it checks `/health` every 5 s. With start at login on and no brain answering for 10 s,
  it starts a detached brain (`DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP`) and logs it.
  With the app quit too, nothing restarts the brain until the next login. That gap is
  accepted (Q12).
- **The app's own child brain** (start at login off): today's rules, with Windows parts.
  `BELLWETHER_PARENT_PID` is opened with `OpenProcess(SYNCHRONIZE)` and waited on; the
  child stops within a second of the app going, even after `TerminateProcess`. A brain the
  app didn't start is never stopped by it.
- **Stopping a brain.** Windows has no SIGTERM for a windowless process. A new `/control`
  action `stop` (tokened, platform-neutral, tested on the Mac too) makes the brain save
  `state.json` and exit. `stop_child` sends it, waits 2 s, then `TerminateProcess`.
- **Log off and shutdown** kill the brain without notice. `state.json` is already saved on
  each change, off the serving thread (M13), so at most the last change is lost.
- **The port.** Windows lets a second socket bind a port held by another unless the first
  used `SO_EXCLUSIVEADDRUSE`. The brain sets it on Windows (through `socket2`). Then a
  second brain fails to bind and exits, as on macOS. Binding `127.0.0.1` raises no
  firewall prompt.
- **After an update** (D5's version check): the app sees `/health` report another version,
  checks with `QueryFullProcessImageNameW` that the brain runs this install's exe, sends
  `stop`, and starts the new one detached. Where `launchctl kickstart` was used, this is
  the one code path that differs.

Single instance:

- The strip: `tauri-plugin-single-instance`, as now (a named mutex from the identifier;
  dev builds keep `local.bellwether`, so dev and installed never collide).
- The brain: the exclusive port bind.

Alternatives:

- **Task Scheduler** (a logon trigger and restart on failure): real crash restarts. But
  restart on failure covers a task that fails to start more than a process that dies
  later, it needs COM (`ITaskService`) or `schtasks.exe` with XML, and it isn't in Task
  Manager's Startup list. Kept as the fallback if supervision proves weak.
- **A Windows service:** needs admin to install, runs in session 0 with no access to the
  user's settings. Rejected.
- **The Startup folder shortcut:** the same as a Run value with more moving parts.
- **`tauri-plugin-autostart`:** writes a Run value through `auto-launch`, but only for the
  app itself, and gives no `StartupApproved` status. A few lines of registry code are
  clearer.

### WD7. The CLI on PATH

**Recommendation: a console-subsystem `bellwether.exe`, built from the same workspace in
the same build and version, installed as `<install>\bin\bellwether.exe`, with `<install>\bin`
added to the user's PATH by the setup window.**

- **Why a second executable:** see Surface. It shares the CLI code (`cli::run`). Its
  `main` is the same three-way dispatch, so `bellwether serve` from a terminal still works.
  `dispatch::mode_of` already treats `bellwether.exe` as the CLI.
- **Why `bin\`:** NTFS is case-insensitive, like APFS. `Bellwether.exe` and `bellwether.exe`
  can't share a directory.
- **How it is bundled:** a `[[bin]]` target in the shell crate, built by the build script
  before `tauri build`, and mapped with `bundle.resources` to `bin/bellwether.exe`. Tauri
  signs the main binary and the installer, not resources, so the build script signs it
  with the same sign script first (WD9). **ASSUMPTION**, checked in W8: `tauri build`
  doesn't also bundle the extra `[[bin]]` at the top level; if it does, it is excluded.
- **PATH:** the setup window's "command-line tool" step shows the exact change (append
  `<install>\bin` to `HKCU\Environment\Path`, kept `REG_EXPAND_SZ`), applies it on
  confirm, and broadcasts `WM_SETTINGCHANGE`. It says that terminals opened before the
  change don't see it. The uninstaller removes the entry (WD9).

Alternatives:

- **`AttachConsole(ATTACH_PARENT_PROCESS)` in the GUI exe:** output appears, but the
  terminal's prompt returns at once and the output lands after it, and exit codes are lost
  to `cmd`. Rejected.
- **A tiny console shim that runs `Bellwether.exe` with its handles:** one real binary,
  but whether a GUI process can write to an inherited console handle, and Ctrl-C for
  `status --watch`, need proving. Kept as the fallback if the second binary is too large.
- **The NSIS installer editing PATH:** NSIS string limits (1024 characters by default)
  truncate long PATH values. Rejected.

### WD8. Files, paths, the token, and Claude Code on Windows

**Paths.**

- **One `user_home()` in `crates/core`:** `%USERPROFILE%` on Windows (`HOME` is often
  unset, and Git Bash sets its own). Every `std::env::var_os("HOME")` in the list above
  goes through it. On macOS it still reads `HOME`.
- **Claude Code's settings:** `%USERPROFILE%\.claude\settings.json`; Claude Code says
  `~/.claude` means `%USERPROFILE%\.claude` on Windows [9]. If `CLAUDE_CONFIG_DIR` is in
  the app's environment, Claude Code keeps its settings there [9], so the installer and
  doctor use it too, on every OS, and doctor names the file.
- **Managed settings** (doctor's checks): `C:\Program Files\ClaudeCode\managed-settings.json`
  and `managed-settings.d\`, `HKLM\SOFTWARE\Policies\ClaudeCode` value `Settings`, and the
  user-writable `HKCU\SOFTWARE\Policies\ClaudeCode` [10]. Today's
  `C:\ProgramData\ClaudeCode\…` is the legacy path, which Claude Code no longer reads [10].
- **App data:** `%APPDATA%\Bellwether\` (already in `home.rs`): `hook-token`,
  `config.json`, `state.json`, `shell-state.json`, `installs.json`, `backups\`, `pets\`.
  **Logs:** `%LOCALAPPDATA%\Bellwether\logs\` (machine-local, never roaming). WebView2's
  data folder is Tauri's default under `%LOCALAPPDATA%\<identifier>\`.
- **Canonical paths:** `installs.json` is keyed by the settings file's real path.
  `canonicalize` on Windows returns `\\?\C:\…`; keys and messages use `dunce`, and keys
  compare case-insensitively on Windows.

**The hook token without 0600.**

- `hook-token` and `backups\` get a **protected DACL** (no inherited entries) that grants
  full control to the current user's SID only, set with `SetNamedSecurityInfoW` when the
  file or directory is created. `persist::create_private_dir`, `write_atomic_sync`,
  `hooks::set_mode` and `token` already have `cfg(not(unix))` branches; this fills them.
- Doctor reads the DACL back and WARNs if anyone else (Users, Everyone, Authenticated
  Users) can read the token.
- The profile's own ACL already keeps other users out of `%APPDATA%`. And the token sits in
  plain text in `settings.json` anyway (D3), so the profile ACL is the real boundary. The
  DACL is defence in depth, like 0600.
- Administrators can always read it. That is the same as root on macOS.

**Claude Code on Windows.**

- **Hooks need no shell.** Bellwether's hooks are `type: "http"` handlers. On Windows,
  command hooks run in Git Bash or PowerShell [11], with quoting and path problems.
  HTTP hooks have none of that. The handler is unchanged: the same URL, 1-second timeout,
  headers, `allowedEnvVars`, and no `onFailure`.
- **Headers:** `WT_SESSION`, `HERDR_PANE_ID` and `HERDR_SOCKET_PATH` are already in
  `ENV_HEADERS`. `TERM_PROGRAM` is set by VS Code and WezTerm on Windows, not by Windows
  Terminal.
- **Line endings and BOM.** A `settings.json` edited in Notepad may have CRLF line endings
  or a UTF-8 BOM. The installer keeps both: it writes the file's own newline and keeps a
  BOM if one was there. New vectors in `installer-vectors/` cover both, on every OS.
- **Atomic writes.** `std::fs::rename` over an existing file fails with access denied or a
  sharing violation while another process has it open without `FILE_SHARE_DELETE`
  (an editor, an antivirus scan, Claude Code's watcher). Writes to `settings.json`,
  `config.json`, `state.json` and the token retry for up to 1 s, then fail with a message
  naming the file. The changed-while-editing sha256 check stays.

### WD9. Packaging, signing, updates, distribution

**Installer: NSIS, per user.**

- `bundle.targets: ["nsis"]` in a Windows overlay (`tauri.windows.dist.conf.json`, merged
  with `--config` like the macOS overlay; deviation 2 of the macOS plan).
  `installMode: "currentUser"`: no admin, installed under `%LOCALAPPDATA%` [3] (the exact
  directory is recorded in W8).
- `webviewInstallMode: downloadBootstrapper` (the default): WebView2 is part of Windows 11
  and current Windows 10 [3].
- `installerHooks` (`NSIS_HOOK_PREUNINSTALL`): unless the uninstaller runs for an update,
  it runs `bin\bellwether.exe hooks uninstall --write` (with its backup) and
  `autostart uninstall --write`, and removes the PATH entry. Without that, every tool
  call would wait for a missing brain and show a hook error. Decided by the maintainer
  (2026-10-20, Q8). Tauri's NSIS uninstaller
  already offers to delete app data.
- **A running brain blocks file replacement.** Tauri's installer finds processes using
  `Bellwether.exe` through Restart Manager and, in passive mode, force-closes them without
  asking [20]. So before an update the app sends the brain `stop` (WD6), and the brain's
  last save is not lost.

**Signing: Azure Artifact Signing, about USD 9.99 a month (decided 2026-10-20), in
Fanzoo's existing Azure account.**

- Basic plan: USD 9.99 a month for 5 000 signatures [8]. A release signs about four files.
- Public-trust certificates are for organisations in listed countries (the US among them)
  and for individuals in the US and Canada [8]. Fanzoo Technology, Inc. validates as an
  organisation. Microsoft quotes 1 to 20 business days for identity validation, longer if
  it asks for documents [8]. **Validation starts now** (by-hand steps in Auth and
  permissions), so it is done well before W9.
- **No account, subscription, tenant, resource-group, account-name or profile ID goes in
  the repository.** They live only in the GitHub `release` environment's secrets and in
  the maintainer's Azure portal.
- Tauri runs it as `bundle.windows.signCommand`. Tauri's docs give
  `artifact-signing-cli -e <endpoint> -a <account> -c <profile> %1`, with
  `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET` and `AZURE_TENANT_ID` in the environment [2].
- **Keeping IDs out of the public repo:** the overlay's `signCommand` is
  `node ../../scripts/sign-windows.mjs %1`. The script reads the endpoint, account and
  profile from the environment and refuses to run without them, so no ID is committed.
- What gets signed: `Bellwether.exe`, `bin\bellwether.exe`, the uninstaller and
  `-setup.exe`. Timestamped. Checked with `Get-AuthenticodeSignature` (Status `Valid`) in
  CI.
- **SmartScreen and Smart App Control.** Since 2024, EV certificates no longer get
  instant SmartScreen reputation; OV and EV both build it over time [2]. A new signed
  release may still show "Windows protected your PC" for a while. Smart App Control on
  Windows 11 blocks unsigned apps outright and may block new signed ones with no
  reputation yet [16]. Signing every release with the same identity is what builds
  reputation. A submission to Microsoft for review is the fallback.

Signing alternatives:

| Route | Cost | Notes |
|---|---|---|
| Azure Artifact Signing | USD 9.99/month [8] | no hardware; works from CI; chosen (Q1) |
| OV certificate from a CA (Sectigo, DigiCert, SSL.com) | a few hundred USD a year, plus cloud-HSM signing fees (**ASSUMPTION**: get current quotes) | since June 2023 keys must live on a token or HSM [2], so CI needs the CA's cloud signing service |
| EV certificate | more than OV | no longer gives instant reputation [2]; no gain here |
| Unsigned | none | SmartScreen warns on every download; Smart App Control blocks it [16]; winget submission becomes harder |

**Updater.**

- `createUpdaterArtifacts: true` signs `-setup.exe` with the **existing** updater key (the
  one in `tauri.dist.conf.json`), producing `-setup.exe.sig` [1]. One `pubkey` for both
  platforms.
- The plugin looks up `windows-x86_64-nsis` first, then `windows-x86_64` in `latest.json`
  (read in its source, `get_urls`) [20]. `latest.json` gets `windows-x86_64` next to
  `darwin-aarch64` and `darwin-x86_64`.
- `plugins.updater.windows.installMode: "passive"`: a small progress window, no questions
  [1]. On Windows the app exits when the installer starts [1]; the installer relaunches it.
- The flow: check (daily and from the tray, unchanged) → ask with `rfd::MessageDialog`
  ("Install and Relaunch" / "Later") → download → `stop` the brain → install (the app
  exits) → the new app starts → it starts the new brain (WD6).
- **The gap:** hooks fail from the brain's stop to the new brain's start, the length of
  the install. That is several seconds, not macOS's sub-second gap (Q7).

**Distribution.**

- **GitHub Releases:** the signed `-setup.exe` and `.sig`, beside the DMG, in the same
  draft release.
- **winget** (recommended second channel): a manifest in `microsoft/winget-pkgs`
  (`InstallerType: nullsoft`, `Scope: user`). A new `winget.yml` on `release: released`,
  like `tap.yml`: it renders the manifest with the version, URL and sha256 and opens a PR
  from a fork, with a fine-grained token in its own `winget` environment. Submissions are
  validated automatically and may be reviewed by hand [13].
- **Scoop, Store:** later (Q5).

**release.yml grows like this:**

1. The existing macOS `build` job is unchanged.
2. A new `build-windows` job on `windows-2025`, in the `release` environment, after the
   same "tagged commit is on main" check:
   - Rust 1.99.0 with `x86_64-pc-windows-msvc`; `npm ci --prefix shell`;
   - `artifact-signing-cli` installed at a pinned version;
   - the Azure secrets and the signing IDs from the environment;
   - `node scripts/build-windows.mjs --sign`;
   - `node scripts/release.mjs assets --windows …`: the installer, its `.sig`, and a
     `latest.windows.json` fragment;
   - checks: `Get-AuthenticodeSignature`, "no hidden commands" (`Bellwether.exe __fixture`
     exits 1 with "unknown command"; also `bin\bellwether.exe`);
   - every temporary key file deleted in an `always()` step.
3. `publish` (still holding no secret) downloads both artifacts, merges the fragments into
   one `latest.json` with `release.mjs merge` (refusing if versions differ or a key is
   missing), writes one `SHA256SUMS`, and drafts the release.
4. `ci.yml` gets a `windows` job with no secrets: tests, an unsigned NSIS build, release
   files with a throwaway updater key (as the macOS `bundle` job does), and the test zip
   (WD12).

### WD10. The setup window and the tray on Windows

- **The same 11 commands and the same capability files.** The steps that don't apply on
  Windows are hidden by `setup_status`, not removed. So the capability tests (which pin
  the exact lists) stay as they are.
  - Earlier install: hidden (no Windows legacy).
  - Start at login: the Run values (WD6), with the "turned off in Task Manager" state.
  - Hooks: unchanged, with the Windows path.
  - Sheep on the Dock, click to jump to its tab: hidden. A line says no permission is
    needed on Windows.
  - Pet: the Import… dialog is `rfd::FileDialog`, from Rust.
  - Command-line tool: the PATH change (WD7).
  - Done: doctor.
- **The tray icon:** `tray-icon` 0.25.1 (already in the tree) in the notification area. A
  colour `.ico` that reads on light and dark taskbars (the macOS one is a monochrome
  template). Windows 11 puts new icons in the overflow menu; the Done step says how to pin
  it. The menu is the same.

### WD11. Dragging a sheep on Windows (M12 and its two-monitor check)

- No player change. `drag.mjs`, the hold and the 250 ms fail-safe are the same.
- `mouse_down()` on Windows: `GetAsyncKeyState` for the left, right and middle buttons.
  It reports physical buttons, which is fine because any button down counts. No
  permission.
- Pointer capture in WebView2 should keep moves and the release coming while the cursor
  is outside the strip (Chromium captures the mouse). M12 step 1 (a) and (b) are re-run on
  Windows in W4 to prove it.
- The cursor-to-strip mapping uses WD3's physical space.
- **M12's unrun two-monitor check happens on the maintainer's Windows PC in W4.** That
  proves the Windows code path and the shared player and placement logic. It does not
  prove the macOS cursor mapping with two monitors. M12's by-hand check 5 stays open for
  macOS until a Mac has a second display; W4's status says so.

### WD12. Testing without a Windows dev machine

Development happens on the maintainer's Mac through agents. The rules that make that work:

1. **Pure first.** Every Windows decision that can be a pure function is one, outside
   `cfg(windows)`, so `cargo test` on the Mac runs it: `taskbar_edge`, the strip's
   physical frame, the `Physical` desktop space and its cursor mapping, the full-screen
   rule, the `StartupApproved` reader (bytes in, state out), the Run-value text, the
   locator's parent walk and window choice (over fixture lists), the DACL decision, the
   newline and BOM handling, the `latest.json` merge. Only thin FFI sits behind
   `cfg(windows)`.
2. **Seams with fakes.** The locator's `Runner` and `Windows` traits get Windows
   siblings: `Processes` (snapshot, environment markers) and the same `Windows`. Tests use
   fakes; a unit test that reaches a real Win32 call panics, as `agent.rs` does for
   launchctl.
3. **`--dump-desktop <file>`** (dev and test builds only, never release): writes the
   monitors (rectangles, work areas, scales, names), the taskbar edge per monitor, the
   foreground window, and every visible top-level window with pid, parent pid, exe name,
   class name, bounds and cloaked state. No titles, command lines or environment. The
   maintainer runs it on his PC and attaches the file to the PR. The developer turns it
   into test fixtures (`test/fixtures/windows/desktop-*.json`).
4. **Windows CI (`windows-2025`), no secrets:**
   - `cargo check` of the whole workspace for Windows, from W1 (today only core and brain
     are checked, from Ubuntu);
   - `npm run test:cargo` and the `conformance` feature tests;
   - `npm test`. Tests that can't run on Windows are skipped through one list in
     `test/helpers.mjs`, each with a reason, and the run prints how many were skipped.
     The count is pinned in CI so it can only fall;
   - the black-box hook suite against `Bellwether.exe serve`;
   - a smoke run: `Bellwether.exe --demo --snapshot … --quit-after 10` with a scratch home.
     If the runner has no interactive desktop, the step says so and is skipped, never
     silently passed;
   - from W8: the unsigned NSIS build and release files with a throwaway updater key.
5. **The test zip.** Every Windows CI run uploads `bellwether-windows-test.zip`: the
   unsigned release exe and `bin\bellwether.exe`, built with a compile-time test port and
   scratch home (the Windows form of `build-app.mjs --test-port --test-home`), and
   `run-test.cmd`. That build never touches `%USERPROFILE%\.claude\settings.json`, never
   writes Run values, and refuses `hooks install` without `--settings <file>`. The
   maintainer uses it for every by-hand check before W8. Real sessions use
   `claude --settings <file from hooks print>`.
6. **Results come back as files:** `shell.log`, `brain.log`, `doctor` output, a
   `--dump-desktop` file, and screenshots (capture exclusion is off by default on
   Windows, so Win+Shift+S shows the sheep).
7. **The maintainer's PC is live too.** By-hand checks never use his real
   `settings.json`, his port 47417 or his `%APPDATA%\Bellwether`, until W10's final check on
   the signed build.

### WD13. Linux

**Recommendation: Windows first; Linux in a later plan of its own.**

- The brain and core already build for Linux (CI checks it). Paths are in `home.rs`.
- The strip is the hard part. On Wayland, an app can't place its own window or read the
  global cursor. It needs `wlr-layer-shell` (not on GNOME) or X11 through XWayland. Every
  desktop is different, so it is a design of its own.
- What this plan does to keep it possible: the `DesktopSpace` seam (Linux X11 is physical
  like Windows), the per-OS login-items seam (XDG autostart or a systemd user unit
  later), and the pure-first rule.

---

## Auth and permissions

- **Windows permissions the app needs at runtime: none.** No elevation, no UAC prompt.
  Placement, the window list, the process list for the user's own processes, the
  foreground call, the registry under HKCU and the opt-in capture exclusion all work as a standard
  user.
- **Reading another process's environment** is limited in code to processes of the same
  user (the token's SID is compared first). Only the marker variable names are matched;
  values other than markers are never kept.
- **Registry writes** are an allowlist in one module: the two `Run` values, and the PATH
  entry under `HKCU\Environment`. Any other key is refused in code (a test lists them). The
  `StartupApproved` key is read only, never written: turning an item back on is the
  user's, in Settings.
- **Settings writes** stay what they are: `hooks install`/`uninstall` only, with preview,
  backup, exact-URL ownership and the sha256 check.
- **What the maintainer does by hand: Artifact Signing in Fanzoo's existing Azure account.**
  Start now: identity validation takes 1 to 20 business days [8]. Steps follow Microsoft's
  quickstart [8]; the portal names below are from it (2026-10-08 revision). Nothing from
  these steps goes into the repository, a PR, an issue or a chat log: the IDs go only into
  GitHub environment secrets (step 9).
  1. In the Azure portal, pick the Fanzoo subscription to use (Subscriptions). Under
     **Resource providers**, register **Microsoft.CodeSigning**.
  2. Search **Artifact Signing Accounts** > **Create**: that subscription, a new resource
     group (for example one just for signing), a unique account name (3–24 letters and
     digits), a supported region (East US, West US 2, …), pricing **Basic**.
  3. On the new account, **Access control (IAM)**: give yourself the **Artifact Signing
     Identity Verifier** role (without it, **New identity** is greyed out).
  4. **Identity validations** > **Organization** > **New identity** > **Public**. Fill in the
     legal name (Fanzoo Technology, Inc.), its website, a monitored primary email *on the
     company's domain* (links expire in 7 days and must be accepted from outside senders),
     a second email on the same domain, the business identifier (for example the EIN or
     state registration number), the business address, and your name exactly as on your
     government ID. Check **Certificate subject preview**, then **Create**.
  5. When it says **Action Required**, follow the emailed link to verify your own identity
     (Microsoft Authenticator and a photo ID). Upload any documents it asks for (issued in
     the last 12 months). Wait for **Completed**.
  6. **Certificate profiles** > **Create** > **Public Trust**: a name (5+ letters and
     digits), **Verified CN and O** = the completed validation. Leave street address and
     postal code off unless you want them in the certificate.
  7. In **Microsoft Entra ID** > **App registrations**, create one app for GitHub CI (for
     example "bellwether-release-signing"). Under **Certificates & secrets**, make a client
     secret with an expiry you will note in your calendar.
  8. Back on the Artifact Signing account (or just the certificate profile), **Access
     control (IAM)**: give that app the **Artifact Signing Certificate Profile Signer** role
     (the signing role in Microsoft's roles tutorial; older portals may still say
     "Trusted Signing"). Nothing else.
  9. In GitHub, repository **Settings** > **Environments** > `release`, add these secrets:
     `AZURE_TENANT_ID`, `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`,
     `ARTIFACT_SIGNING_ENDPOINT` (the region's URL, such as
     `https://eus.codesigning.azure.net`), `ARTIFACT_SIGNING_ACCOUNT`,
     `ARTIFACT_SIGNING_PROFILE`.
  10. Tell the developer only "signing is ready". W9's first tagged pre-release proves it.
- **For winget (W10):** fork `microsoft/winget-pkgs` under the Fanzoo account, make a
  fine-grained token that can open PRs from that fork, and store it as
  `WINGET_GITHUB_TOKEN` in a new `winget` environment (deployment rule: `v*` tags).

## Configuration

No new `config.json` keys. Existing keys that start doing something on Windows:

| Key | Example | On Windows |
|---|---|---|
| `privacy.captureExclusion` | `false` (the Windows default; macOS keeps `true`) | opt-in: `WDA_EXCLUDEFROMCAPTURE` on every strip, which also hides the sheep from your own screenshots |
| `privacy.hideWhenSharing` | `true` | hide while a `sharingProcesses.win32` process runs |
| `privacy.hideWhenFullscreen` | `true` | the WD5 detector (unused on macOS) |
| `privacy.sharingProcesses.win32` | `["CptHost.exe"]` | the Toolhelp process names |
| `locator.strategies` | `["herdr", "process-tree", "vscode", "app-activate"]` | the Windows default order; macOS-only names are skipped with one log line |
| `locator.herdrPath` | `"herdr"` | resolved on PATH, then `%LOCALAPPDATA%\Programs\Herdr\bin\herdr.exe` |

Environment variables read (unchanged unless marked):

| Variable | Example | Use |
|---|---|---|
| `BELLWETHER_HOME` | `C:\Users\jeff\AppData\Local\Temp\bw-test` | test home |
| `BELLWETHER_CONFIG` | `…\config.json` | read-only test config |
| `BELLWETHER_HOOK_TOKEN` | (tests, CI) | overrides the token file |
| `BELLWETHER_PARENT_PID` | `4242` | the child brain's watchdog |
| `USERPROFILE` (**new use**) | `C:\Users\jeff` | the user's home on Windows |
| `CLAUDE_CONFIG_DIR` (**new use**, all OSes) | `D:\claude-config` | where Claude Code keeps settings |

Release secrets (in GitHub environments, never in the repository): the six names in Auth
and permissions step 5, `WINGET_GITHUB_TOKEN`, and the existing
`TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` (the same key signs
both platforms' updates).

## Data model

**`latest.json`, merged** (one file for both platforms):

```json
{
  "version": "1.2.0",
  "notes": "…",
  "pub_date": "2026-11-…",
  "platforms": {
    "darwin-aarch64": { "url": "…/Bellwether.app.tar.gz", "signature": "…" },
    "darwin-x86_64":  { "url": "…/Bellwether.app.tar.gz", "signature": "…" },
    "windows-x86_64": { "url": "…/Bellwether_1.2.0_x64-setup.exe", "signature": "…" }
  }
}
```

`release.mjs merge` refuses fragments with different `version`s, a missing platform, or a
URL that isn't in the release's files.

**Run values** (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, `REG_SZ`):

- `Bellwether` = `"C:\Users\<u>\AppData\Local\<install dir>\Bellwether.exe"`
- `Bellwether Brain` = `"…\Bellwether.exe" serve`

**`StartupApproved\Run`** (read only): a `REG_BINARY` per value name; first byte `02` on,
`03` off, anything else `unknown` [17].

**`desktop.json`** (`--dump-desktop`, dev and test builds):

```json
{
  "version": 1,
  "windows": "10.0.26100",
  "monitors": [
    { "name": "\\\\.\\DISPLAY1", "primary": true, "scale": 1.0,
      "rect": [0, 0, 1920, 1080], "work": [0, 0, 1920, 1032], "taskbar": "bottom" }
  ],
  "foreground": 132456,
  "processes": [ { "pid": 9100, "ppid": 8800, "exe": "claude.exe" } ],
  "windows": [
    { "hwnd": 132456, "pid": 8800, "exe": "WindowsTerminal.exe",
      "class": "CASCADIA_HOSTING_WINDOW_CLASS", "bounds": [1930, 10, 3800, 1400],
      "cloaked": false, "tool": false }
  ]
}
```

No titles, command lines or environment values, with one exception: for `herdr.exe`
processes only, the first argument if it is a herdr subcommand word (such as `server`),
never a path or a value (WD4). Rectangles are physical pixels.

**Files on disk:** the same names and shapes as macOS (D3), at the WD8 paths.
`installs.json` keys are `dunce` paths, compared case-insensitively on Windows.

## Milestones

The order puts the riskiest unknown first: does the overlay itself behave on Windows
(transparent, click-through, no focus theft, not throttled, placed right with mixed
DPI)? Then real sessions as early as possible (W2), so something useful runs on the
maintainer's PC before any packaging.

Every milestone ends with a status section and deviations, as M1–M13 did. By-hand checks
use the test zip (WD12) until W8, and the installed test build after.

### W1. The strip on Windows: a demo flock on the taskbar

Builds:

- `cfg(windows)` stubs so the whole shell crate compiles for Windows; the Windows CI job
  with `cargo check --workspace --all-targets` and `npm run test:cargo`;
- strip frames in physical pixels from each monitor's work area, `ScaleFactorChanged`
  handling, the 2 s re-read (WD1, WD3);
- `WS_EX_TOOLWINDOW`; the heartbeat measure for WebView2 throttling, with the
  browser-arguments fallback behind a constant (WD2);
- `taskbar_edge` and its log line;
- `--dump-desktop`;
- the test zip with `run-test.cmd` (`--demo`).

**Done when:**

- the Windows CI job is green: the workspace checks and `npm run test:cargo` pass on
  `windows-2025`;
- on the maintainer's PC, `run-test.cmd` shows the demo flock on every monitor, the sheep's
  feet on the taskbar's top edge (a screenshot of each monitor);
- a click in the strip's band away from a sheep reaches the window underneath; a click on
  a sheep makes it hop, logs `sprite clicked`, and typing in Notepad carries on (focus
  never moved);
- the strip is in neither the taskbar nor Alt-Tab nor Task View;
- after 10 minutes, including Win+D, a locked and unlocked screen, and a maximised window
  on the same monitor, the log's heartbeat never stopped;
- the pure tests (taskbar edge, physical frames, mixed-scale cases) pass on the Mac and on
  Windows.

**By hand (the maintainer, about 20 minutes):**

1. Run `winver` and note the Windows 11 version and OS build it shows (for W1's status and
   Q14). Then set one monitor to 100% and the other to 150% (Settings > Display > Scale).
   Run `run-test.cmd`. A screenshot of each monitor's bottom edge.
2. Turn "Show my taskbar on all displays" on and off: each strip follows within 2 s.
3. Turn "Automatically hide the taskbar" on: the strips drop to the screen's bottom edge
   within 2 s. Off again.
4. Change monitor 2's scale while it runs: its strip re-fits within 2 s, at the right size.
5. Move the cursor slowly across several sheep: any flash or flicker? (WD2 step 3.)
6. Win+Tab, make a new desktop, switch to it: is the strip there? Switch back. (WD2.)
7. Play a video full screen (F11) on monitor 2: record whether the strip shows over it
   (expected, until W5).
8. Win+Shift+S a region over the strip: the sheep **are** in the capture (capture
   exclusion is off by default on Windows).
9. Run `run-test.cmd --dump-desktop desktop.json` and attach the file and both logs.

#### W1 status (developer, 2026-10-19, updated 2026-10-21): built; the cargo suite green on Windows in a VM; CI green; first by-hand run on the maintainer's PC done, checks 2, 4, 13 and the new 14 open

**Update, 2026-10-21: the maintainer's PC.** Windows 11 26H2, OS build 26300.9711, x64. Two
monitors: the primary 5120x2160 at 150%, and a portrait 2160x3840 at 125% at x 5120, y
-934 (above and right of the primary's top). `run-test.cmd` (port 47491): both strips came
up on the taskbars' top edges (`strip-0 … x 0 y 1878 5120x210`, `strip-1 … x 5120 y 2671
2160x175`, physical px), clicks, the drag between monitors and the heartbeat (every 10 s,
no silence, over about 2 minutes) were right. One bug: at one point the sheep were behind
ordinary windows, not reproduced with Win+D or a lock. Fixed by W1-15 (the strips are put
back on top every 2 s and on every foreground change, and the log says when that was
needed); its by-hand check is 14 below. Open from that run, for the next PC test with
this fix: checks 2 (taskbar on all displays), 4 (scale change while running) and 13 (10
minutes with Win+D and a lock). Not a bug: with `--demo-monitors` every monitor shows the
whole demo flock, so dragging a sheep moves its copy on the other monitor too (W1-5; a
note is under the by-hand list). Real sessions put each sheep on one monitor.

W1-15 in the VM (2026-10-21, the rebuilt test folder): a probe script dropped the strip's
topmost with `SetWindowPos(HWND_NOTOPMOST)`. Alone, the 2 s re-read put it back and logged
`strip-0: was no longer on top (WS_EX_TOPMOST gone, seen on the 2 s re-read); put back on
top`; together with a new Explorer window coming forward, it was back within about 160 ms,
twice, logged `… seen on a foreground change …`. Before and after, the strip's extended
style kept `WS_EX_TOOLWINDOW`, `WS_EX_TRANSPARENT`, `WS_EX_LAYERED` and `WS_EX_NOACTIVATE`,
and the strip was never the foreground window. The cargo suite in the VM: see W2's status
(the same run).

**Update, 2026-10-20.** The maintainer ran the test folder in a Windows 11 ARM64 Parallels
VM on this Mac (x64 build under emulation): placement, drag, clicks and the heartbeat
looked right; the sheep were missing from screenshots until W1-10, and are there since.
The coordinator then ran `cargo test --workspace --no-fail-fast` natively in that VM
(Windows 11 25H2, build 26200; Rust 1.99.1, target `x86_64-pc-windows-msvc`; VS 2022
Build Tools 14.44 and SDK 26100; scratch USERPROFILE, APPDATA and LOCALAPPDATA): 254
passed, 5 failed, one target never ran (W1-11). After the fixes, the same run in the VM:
**257 passed, 0 failed**, every test target run (21 executables and the doc tests). The
Mac runs 304: the 47 more are the Unix-only tests (W1-7, W1-11). On the Mac after W1-11:
`npm run test:cargo` 304 passed, 0 failed; `npm test` 1037 tests, 1036 pass, 0 fail, 1
skipped (the exe-name guard is new). This is not CI: the `windows-2025` job is still
unrun, and the VM is ARM64 with one virtual display, not the two-monitor x64 PC the
by-hand list is for.

After BR-3K's W1 fixes (W1-12 to W1-14): the VM's cargo run **258 passed, 0 failed**
(every target; the new `PhysRect::matches` test included); on the Mac
`npm run test:cargo` 305 passed, 0 failed, and `npm test` 1037 tests, 1036 pass, 0 fail,
1 skipped; the xwin check clean. In the VM, with the rebuilt folder:
`run-test.cmd --dump-desktop Z:\nope\x.json` printed `Could not write … (exit code 1)`
and the log's line `--dump-desktop: cannot write Z:\nope\x.json: The system cannot find
the path specified. (os error 3)`, and exited 1; a dump to a real folder wrote the file
(1 monitor, 149 processes, 17 windows), logged it and exited 0. The fresh-log move was run
with a stand-in exe (no window): the old `shell.log` became `shell.previous.log`. The
deferred re-fit after a scale change has not run on Windows (by-hand step 4).

Before that update: the code compiled and linked for
`x86_64-pc-windows-msvc` on the Mac (cargo-xwin, below), and the pure parts are tested on
the Mac. Whether the Windows CI job is green, and everything in the by-hand list, is
unproven until CI runs on `windows-2025` and the maintainer runs the list on his PC.

Built:

- **The whole shell crate compiles for Windows.** `icons/icon.ico` (deviation W1-1). The
  shell's `libc` calls outside tests go through `sys.rs`, with Windows stand-ins until W2,
  W3 and W6 (deviation W1-8). Tests that need a shell script, `/bin` programs, symlinks, a
  FIFO, Unix permission bits, launchd or LaunchAgents are `#[cfg(unix)]`, each with its
  reason (deviation W1-7).
- **Strip frames in physical pixels** (WD1, WD3): `desktop.rs` (pure): `taskbar_edge`,
  `physical_strip_rect` (the work area's bottom minus `strip.heightPx × scale`), and the
  `Space` seam (`Points`, today's macOS maths, unchanged; `Physical` on Windows).
  strips.rs sets a Windows strip with `PhysicalPosition`/`PhysicalSize` before it is shown,
  re-reads every strip's monitor every 2 s (the privacy loop's Dock check, every pass on
  Windows), puts a strip back to its frame on `ScaleFactorChanged`, and logs each monitor's
  taskbar edge when it is first seen and when it changes (`taskbar on monitor
  "\\.\DISPLAY1": bottom (screen …, work area …, physical px, scale 1)`). The cursor poll,
  `cursor_now` (the drag) and the monitor list go through the seam, so the cursor maps to
  the right strip with mixed scales (WD3's example is a test).
- **`WS_EX_TOOLWINDOW`** on every strip, kept by a window subclass on every style change
  (`win.rs`; deviation W1-2). Logged as `strip-0: a tool window (WS_EX_TOOLWINDOW, kept on
  every style change): not in the taskbar, Alt-Tab or Task View`.
- **The WebView2 throttling measure** (WD2 step 2, WR1): `--heartbeat`. Each strip's page
  logs `heartbeat N: <ticks> ticks in <s> s, longest gap <ms> ms, page <visibility>, <n>
  sprites` every 10 s from a 1 s timer, and the shell logs `heartbeat: strip-0 silent for N
  s: its page is throttled or suspended (WR1)` after 25 s without one, and `… is back after
  N s of silence` (`heartbeat.rs`; deviation W1-6). The browser-arguments fallback is
  `strips::WEBVIEW2_UNTHROTTLED` (false) and `webview2_args`, applied to the strips and the
  setup window alike when it is turned on.
- **`--dump-desktop <file>`** (WD12 3): `dump.rs` (the file's shape, tested on the Mac) and
  `win.rs` (the collection). Development builds and the test build only (`dump::allowed`).
  Fixture: `test/fixtures/windows/desktop-synthetic-100-150.json` (deviation W1-4).
- **Capture exclusion off by default on Windows** (brought forward from W5 after the
  maintainer's first run; deviation W1-10): `privacy::capture_exclusion`, config.json's
  value wins on any OS, the log says which and why.
- **`--demo --demo-monitors`**: a strip on every monitor, each with the whole demo flock
  (deviation W1-5). `--demo` alone is unchanged.
- **The test folder** (WD12 5): `scripts/build-windows-test.mjs` builds `Bellwether.exe`
  (release profile, `--features test-build`), `run-test.cmd`, `config.json` on port 47491,
  the two bundled pets and a README (deviation W1-3).
- **CI** (`ci.yml`, job `windows`, `windows-2025`, no secrets): LF checkout; `cargo check
  --locked … --workspace --all-targets`; `npm run test:cargo -- --no-fail-fast`; the test
  folder; a 25 s smoke run of its demo (`::warning` and skipped if no strip opens, never
  passed silently; fails if the tool-window, taskbar or `--quit-after` lines are missing);
  `--dump-desktop` on the runner; the folder uploaded as the `bellwether-windows-test`
  artifact (GitHub zips it: `bellwether-windows-test.zip`).
- **Docs:** docs/DEVELOPMENT.md "Windows (in progress)", the script table and the CI
  paragraph.

How the Windows code was compiled here: the Mac has `x86_64-pc-windows-msvc` (rustup).
`cargo check --target x86_64-pc-windows-msvc` alone stops at `ring`'s C build (no Windows
headers), so `cargo-xwin` 0.23.1 (`cargo install cargo-xwin`, which downloads Microsoft's
CRT and SDK headers) with Homebrew's LLVM and LLD 23.1.3 (`clang-cl`, `llvm-lib`,
`lld-link`): `cargo xwin check … --workspace --all-targets --target x86_64-pc-windows-msvc`
passes with no warnings, and `node scripts/build-windows-test.mjs --xwin` links a 12.6 MB
`PE32+ executable (GUI) x86-64`. Nothing Windows can run here: no test and no exe ran
on Windows.

Tested here (macOS):

- `npm run test:cargo`: 304 passed, 0 failed (290 before; 14 new: desktop.rs 5, the
  Windows cursor-mapping table in clickthrough.rs 3, dump.rs 3, heartbeat.rs 2, and the
  per-OS capture-exclusion default in privacy.rs 1, after W1-10). Then
  `npm run build:dev` and `npm test`: 1036 tests, 1035 pass, 0 fail, 1 skipped (new:
  `test/windows.test.mjs`, 8 tests; `test/docs.test.mjs` now allows the developer doc to
  name docs/WINDOWS-PLAN.md; the two `the example config` vectors in
  `test/fixtures/config-vectors.json` lose `captureExclusion`, as config.example.json did,
  W1-10). Held awake with `caffeinate -i`. Rerun after W1-10: the same counts.
- macOS unchanged: a 23 s `--demo --demo-monitors --heartbeat --snapshot` run of the
  development build (scratch home, port 47431, one display) logs the same strip lines as
  before (`strip strip-0 on monitor "Monitor #41053": x 0 y 911 1728x140 (logical px)`, no
  taskbar or desktop-space line) and `webview: heartbeat 1: 10 ticks in 10.0 s, longest gap
  1010 ms, page visible, 13 sprites`, then quits.
- `actionlint` 1.7.12 on ci.yml: clean. The job's two PowerShell steps parse with `pwsh`
  7 on the Mac, and were run against a stand-in `Bellwether.exe` (a shell script writing a
  log, and the fixture as the dump): the pass, the "no strip opened" warning and skip, and
  the missing-line failure all behave.

**What only a CI run on `windows-2025` can prove:** that `cargo check` and every cargo
test pass on Windows (the static pass above found the Unix-only tests it could see; any
other test that assumes Unix paths or programs will show in the first run, all at once
with `--no-fail-fast`); that the test folder builds natively with MSVC; that the exe
starts, opens strips, logs the tool-window and taskbar lines and quits (if the runner has
a desktop); and that `--dump-desktop` writes a valid file.

**What only the maintainer's PC can prove:** everything in the by-hand list below.

**By hand (the maintainer, about 25 minutes, on the Windows 11 PC with two monitors).**
Never on the installed app: the test folder has its own home and port and starts only the
demo.

Before you start: open the latest green `ci` run of this branch on GitHub, and under
**Artifacts** download `bellwether-windows-test`. Right-click the zip > Properties > tick
**Unblock** > OK, then Extract All, for example to `Downloads\bellwether-windows-test`.
Open Windows Terminal in that folder. If Windows says it protected your PC when the app
starts (it is unsigned), click More info > Run anyway.

1. Press Win+R, type `winver`, Enter. Write down the version and OS build it shows (for
   example "Version 24H2 (OS Build 26100.4652)"). Then in Settings > System > Display,
   set one monitor to 100% and the other to 150% (Scale). Run `.\run-test.cmd`. The demo
   flock appears on both monitors, the sheep's feet on the taskbar's top edge. Take a
   screenshot of each monitor's bottom edge (Win+Shift+S, then paste into Paint and save).
2. Settings > Personalization > Taskbar > Taskbar behaviors: turn **Show my taskbar on
   all displays** on, then off. Each time, each monitor's strip moves to the taskbar's
   top edge (or the screen's bottom, where there is no taskbar) within 2 seconds.
3. In the same place turn **Automatically hide the taskbar** on: the strips drop to the
   screens' bottom edges within 2 seconds. Turn it off again: they come back up.
4. Change the second monitor's scale (for example 150% to 125%, then back) while it runs:
   its strip fits again within 2 seconds, the sheep the right size, standing on the
   taskbar.
5. Move the mouse slowly across several sheep, on both monitors. Note any flash or
   flicker of the strip.
6. Press Win+Tab, click **New desktop**, switch to it. Is the strip there? Switch back.
7. Play a video full screen (F11 in a browser, or a video's full-screen button) on the
   second monitor. Note whether the strip shows over it (expected for now; W5 hides it).
8. Win+Shift+S, drag a region over the strip: the sheep **are** in the capture.
9. Quit the demo (the sheep icon in the notification area, which may be under ^, then
   **Quit Bellwether**; or wait: it quits by itself after 30 minutes). Run
   `.\run-test.cmd --dump-desktop desktop.json`. Attach `desktop.json` and
   `home\shell.log` (there is no `brain.log` in W1: the demo starts no brain). Each demo
   run starts a fresh `shell.log` (the run before is kept as `shell.previous.log`), so
   attach the log of the run you are reporting on.

The rest of W1's Done-when, in the same run (start `.\run-test.cmd` again if it was quit):

10. Open Notepad and put its window partly behind a strip. Click in the strip's band away
    from any sheep: the click reaches Notepad (or whatever is underneath).
11. Type in Notepad, then click a sheep: it hops, and typing carries on in Notepad without
    clicking it again (focus never moved). `home\shell.log` has `sprite clicked`.
12. The strip is not in the taskbar, not in Alt-Tab (hold Alt, press Tab), and not in Task
    View (Win+Tab).
13. Leave it running 10 minutes. During that time press Win+D (and again), lock the screen
    (Win+L) and unlock it, and maximise a window on the same monitor as a strip. Afterwards,
    in `home\shell.log`, the `webview: heartbeat` lines keep coming every 10 s for each
    strip, and no line says `silent`. If one does, the page was throttled: say so, with the
    log.

14. **The sheep stay on top** (W1-15). While it runs, click the taskbar, open Start and close
    it, open the notification centre (Win+N) and close it, and switch between a few windows
    with Alt-Tab, each time with an ordinary window (Notepad, a browser) maximised on the
    strip's monitor. The sheep stay in front of that window every time. If they ever go
    behind it, note what you had just done, and whether they came back by themselves within
    2 seconds. Then, in `home\shell.log`, look for `put back on top` or `could not be put
    back on top` lines: send them with what you did just before. (At most one such line a
    minute per strip; it says how many more there were.) Also, once, restart Windows
    Explorer (Task Manager > Processes > Windows Explorer > Restart) while the demo runs:
    the sheep are in front of ordinary windows again within 2 seconds.

A note on `--demo-monitors`: every monitor shows the **whole** demo flock (the same demo
sheep on each), so dragging a sheep on one monitor moves its copy on the other too. That is
the demo, not a bug (W1-5). With real sessions (W2 on) each sheep is on one monitor only.

Send back: the winver text, the screenshots (step 1, step 8), notes for steps 2–7 and
10–14, `desktop.json` and `home\shell.log`. Afterwards you can delete the folder and
`%LOCALAPPDATA%\local.bellwether.p47491` (WebView2's files for the test build).

### W2. The brain and the CLI on Windows: real sessions

Builds:

- `user_home()` in core, and every `HOME` read through it (WD8);
- `CLAUDE_CONFIG_DIR` for the settings path (all OSes);
- the protected DACL for the token and backups, and its doctor check;
- write retries for `rename`; CRLF and BOM kept by the installer, with new vectors;
- the parent watchdog by process handle; the `/control` `stop` action and `stop_child` on
  Windows; `SO_EXCLUSIVEADDRUSE`;
- doctor on Windows: port owner, process path, local time, the correct managed-settings
  file and both registry policy values, a Windows legacy section that says "none";
- the console `bin\bellwether.exe` (WD7) in the test zip;
- the Windows `npm test` run with its pinned skip list, and the black-box hook suite
  against `Bellwether.exe serve`.

**Done when:**

- Windows CI runs `cargo test --workspace` (with and without `conformance`), `npm test`
  (skip count printed and pinned) and the black-box hook suite, all green;
- macOS CI is unchanged and green;
- on the maintainer's PC, with the test zip: `bellwether version`, `bellwether doctor` and
  `bellwether status` print in the terminal with correct exit codes; `icacls` on
  `hook-token` shows only his user (with no inherited entries);
- with the test brain and strip running, `bellwether hooks print > hooks.json` and
  `claude --settings hooks.json`, once in a plain Windows Terminal tab and once in a herdr
  pane inside Windows Terminal: a sheep walks on, a permission prompt turns it to face him
  with a bubble, `/exit` makes it leave; in herdr the bubble's label shows the herdr pane
  (as `my-project · p2` does on macOS);
- `bellwether hooks install --settings <scratch copy>` then `--write`: the diff, the
  backup, CRLF kept if the copy had it.

**By hand (about 15 minutes):** the last three Done-when items, plus: quit the strip from
the tray while a session runs, and the session shows no hook error (the brain kept
answering); stop the test brain with `bellwether shell stop` and check `state.json` was
saved (the session's colour comes back on restart).

#### W2 status (developer, 2026-10-21, fix round 2026-10-22): first part built and verified by BR-3K (ship with fixes; fixed, W2-14 to W2-19); cargo suites green on the Mac and in the VM; full `npm test` on Windows, the CI run and the PC checks open

Built:

- **`user_home()` and `CLAUDE_CONFIG_DIR`** (WD8): `crates/core/src/paths.rs`. Every read of
  the user's home goes through it (`USERPROFILE` on Windows, `HOME` elsewhere; W2-12), and
  Claude Code's settings file is `$CLAUDE_CONFIG_DIR/settings.json` when that is set, on
  every OS (the installer's default, doctor, the out-file guard, the setup window's
  `Ctx::claude_dir`). The Node test helper drops an inherited `CLAUDE_CONFIG_DIR`, so a test
  never reaches the developer's own Claude Code.
- **The brain on Windows** (WD6): `POST /control {"action": "stop"}` (tokened, every OS;
  the brain answers, saves state.json and exits; divergence 26); the parent watchdog by
  process handle (`OpenProcess(SYNCHRONIZE)` and a wait, after checking the pid is still
  this process's parent and older than it); `SO_EXCLUSIVEADDRUSE` on the listener
  (`crates/brain/src/win.rs`). The strip stops its own brain with the tokened stop, then
  `TerminateProcess` after 2 s, and starts it without a console window (W2-11).
- **The protected DACL** (WD8): `crates/core/src/acl.rs` (W2-7). Doctor's token line reads
  it back: PASS `… (only you can read it)`, or WARN naming who else can, with the `icacls`
  fix.
- **Rename retries** for up to 1 s on Windows' access-denied and sharing-violation errors
  (W2-13). **CRLF was already kept**; **a UTF-8 BOM is now kept too**, with a new installer
  vector, `utf-8-bom-crlf` (W2-6).
- **Doctor on Windows**: the port's owner (`GetExtendedTcpTable`, the exe's path), the
  answering brain's path, local time, the managed file under `%ProgramFiles%\ClaudeCode`
  with its `managed-settings.d\` drop-ins and the `HKLM`/`HKCU` policy values (W2-9),
  `legacy agents: none`, a Windows identity line; a closed port reads as "not running"
  (W2-8); the bundled pets found in the test folder (W2-10).
- **The console `bin\bellwether.exe`** (WD7; W2-1) and **the test folder for real
  sessions**: `run-test.cmd --sessions` (token, `hooks.json`, the strip and its brain) and
  `cli.cmd` (W2-3), with the test build's own refusals (W2-2).
- **Labels**: a Windows `cwd` is named after its last folder (W2-4; divergence 28).
- **CI** (`windows` job, not yet run): the cargo tests with the `conformance` feature, the
  black-box hook suite against this build's `serve`, and the console CLI's output, exit
  codes and refusals.

Tested:

- Mac: `npm run test:cargo` 317 passed, 0 failed (305 before); the xwin check of the whole
  workspace for `x86_64-pc-windows-msvc` (`--locked`), no warnings. `npm test`: 1020 tests,
  1018 pass, 0 fail, 1 skipped, and `test/brain-server.test.mjs` cancelled at the 120 s
  per-file limit (its 22 tests uncounted; the run before, the same). That file is near the
  limit without W2: run alone as `npm test` runs it, it passed in 106.6 s on the W1 commit's
  own binary and in 107.4 s with W2 (22 of 22 each). Not a regression, but its margin is
  thin on this Mac; raising its timeout or splitting it is for whoever touches it next.
- The Windows 11 ARM64 VM (x64 build under emulation): `cargo test --workspace` **270
  passed, 0 failed**, 24 targets (258 before), and with `--features
  bellwether-shell/conformance` **272 passed, 0 failed**; among them the Windows-only DACL
  round trip and the `/control stop` test. The black-box hook suite
  (`test/hook-route-blackbox.test.mjs`) against the conformance build: 22 passed, 1
  skipped (the 256-file-limit case), 0 failed. The test folder: `cli.cmd version` printed
  `bellwether 1.1.1 (rust)` (exit 0); `hooks install --write` refused (exit 3);
  `status --port 47417` refused (exit 3); `token init` and `hooks print --out hooks.json`
  wrote their files (`only you can read it`); `status` with no brain exit 2. The strip
  started its brain (`started the brain: …\Bellwether.exe serve --port 47491`), four hooks
  posted as a Windows Terminal session in a herdr pane made one sheep with a permission
  bubble (`status`: `1 sheep … ALERT`), a second `serve` on the port exited 2 (`port 47491
  on 127.0.0.1 is already in use`), `SessionEnd` made it leave, and quitting the strip
  logged `asked to stop (POST /control stop); stopping` in brain.log, the brain gone and
  state.json written. The same run with the final build: the sheep labelled `my-project`
  (W2-4); `doctor --settings <a hooks.json copy>` with the brain up: `All checks passed`,
  `PASS token … (only you can read it)`, `PASS brain pid 5288, … last 12:43:57: the Rust
  brain (C:\bellwether-test\w2\Bellwether.exe)` (local time, the exe's path); with no brain,
  `FAIL brain  brain not running …` (W2-8); `icacls home\hook-token`: only the user, `(F)`,
  no inherited entry. `run-test.cmd --sessions`, then the strip ended with
  `Stop-Process -Force` (`TerminateProcess`): the brain was gone 98 ms later, logging `the
  app that started this brain (pid 13000) is gone; stopping`. No real Claude Code ran in the
  VM (its user's own `.claude` is off limits): sessions were hooks posted with the
  Windows Terminal and herdr headers.
- Still macOS wording on Windows, for W3 with the locator lines (BR-3K's W1 observation
  3): doctor's `herdr CLI` (`… through iTerm2/Terminal only`), `accessibility` (System
  Settings) and `next` (`… appears above the Dock`).

**What W2 still needs (its second part):**

- the full `npm test` on Windows with its pinned skip list (WD12 4): today only the
  black-box hook suite runs there; `test/impl.mjs` drives the conformance session over
  FIFOs (`mkfifo`), and other suites assume Unix programs;
- ~~the Windows CI job's first run with W2~~ and ~~the macOS CI run~~: both green since
  (2026-10-28, PR #3 and the 1.2.0 commit);
- the PC checks below.

**By hand on the maintainer's PC (about 20 minutes), with the CI test folder.** Never the
installed app or your own settings: the folder has its own home and port 47491, and
`claude --settings <file>` adds the hooks for one session only.

1. In the folder, in Windows Terminal: `.\cli version`, `.\cli doctor`, `.\cli status`.
   Each prints in the terminal; note the exit codes (`echo $LASTEXITCODE`: version 0;
   doctor 2 or 3 (no brain, hooks not in your settings); status 2 with no brain).
2. `.\run-test.cmd --sessions`. It prints the `claude --settings …\hooks.json` line and
   starts the strip; `home\logs\brain.log` says `listening on http://127.0.0.1:47491`.
3. `icacls home\hook-token`: only your user, no `(I)` (inherited) entries. `.\cli doctor`
   now has `PASS token … (only you can read it)` and `PASS brain`.
4. In a new Windows Terminal tab (no herdr): `claude --settings <folder>\hooks.json`. A
   sheep walks on (on the primary monitor: placing it on the terminal's monitor is W3),
   labelled with the project folder's name. Ask for something that needs a permission: the
   sheep turns to face you with a bubble. `/exit`: it leaves.
5. The same inside herdr in Windows Terminal, twice in the same project folder (two herdr
   panes): two sheep, labelled `<project> · p<n>` with each herdr pane. (One sheep alone
   in its project keeps the plain name.)
6. Real use puts each session's sheep on **one** monitor (not on every monitor as the demo
   does).
7. Quit the strip from the tray (Quit Bellwether): the brain stops with it
   (`asked to stop (POST /control stop)` in brain.log). Start `.\run-test.cmd --sessions`
   again: a session's sheep keeps its colour (state.json was saved).
8. A brain that outlives the strip: in one tab `.\cli serve` (it stays in the foreground),
   then `.\run-test.cmd --sessions` in another (the strip uses that brain and starts none),
   and a `claude --settings …` session in a third. Quit the strip from the tray: the session
   shows no hook error (the brain kept answering). Ctrl-C in the `serve` tab: it logs
   `Ctrl-C, stopping` and saves state.json.
9. The strip ended hard: with `--sessions` running, end `Bellwether.exe` (the one without
   `serve`) in Task Manager: the `Bellwether.exe serve` process is gone within a second.
10. The installer, on a scratch copy: `copy %USERPROFILE%\.claude\settings.json scratch.json`
    (or any settings file), then `.\cli hooks install --settings scratch.json`: the diff;
    `… --write`: `home\backups\settings-….json` written, the file has Bellwether's hooks,
    and if the copy had CRLF line endings or a BOM (open it in Notepad: the status bar says
    `Windows (CRLF)` and `UTF-8 with BOM`) it still has them. `.\cli hooks uninstall
    --settings scratch.json --write` gives back the bytes. `.\cli hooks install --write`
    without `--settings` is refused (exit 3).

Send back: the outputs of steps 1 and 3, notes for 4–10, `home\shell.log` and
`home\logs\brain.log`.

### W3. Which monitor, and clicking a sheep

Builds:

- the Windows `Processes` seam (Toolhelp, the environment marker read for the same user)
  and the Windows `Windows` seam (`EnumWindows`, DWM bounds and cloaking);
- the parent walk, the window choice, `vscode` and `app-activate` for Windows (WD4);
- the `herdr` strategy on Windows: the client-based host walk, the server/client rule,
  `WT_SESSION` ignored when `HERDR_PANE_ID` is present, the pipe-name check for
  `HERDR_SOCKET_PATH`, the herdr path fallback, focus through herdr's CLI (WD4);
- `SetForegroundWindow` with `SW_RESTORE`, the flash fallback, the log lines;
- fixtures from W1's `desktop.json` and new dumps; unit tests over them;
- the Windows log without macOS wording (BR-3K's W1 observation 3): today Windows logs
  `dock: Accessibility not granted to this binary …`, `locator: … (AppleScript on: iTerm2
  and Terminal may ask for Automation permission)` and the macOS strategy chain
  (`herdr > iterm2 > terminal-app …`); on Windows these say what is true there (no
  permission needed; the Windows chain, `locator.strategies` defaulting to the Windows
  order).

**Done when:**

- **herdr first** (the maintainer's setup): with herdr attached in a Windows Terminal
  window on monitor 2 and two Claude Code sessions in different herdr tabs, both sheep
  live on monitor 2 within `strip.relocateSec`; move that Windows Terminal window to
  monitor 1 and they follow within `strip.relocateSec`;
- a click on a herdr session's sheep, while another herdr tab is shown and Notepad has
  focus, brings the Windows Terminal window forward with that session's herdr workspace,
  tab and pane selected; the log says `herdr (…, pane …)`;
- with herdr detached (the client closed, the server still running), the sheep stay on
  the primary monitor and a click logs that no host window was found; after
  re-attaching, the next relocate finds it again;
- outside herdr, with a plain Claude Code session in another Windows Terminal window on
  the other monitor, its sheep lives on that monitor, and a click brings that window
  forward and restores it if minimised; the log names the strategy;
- with two Windows Terminal windows, the sheep goes to the right window's monitor (exact,
  through `WT_SESSION`), and the click brings that window forward;
- if the foreground is refused, the taskbar button flashes and the log says
  `foreground refused`;
- the locator's Windows tests pass on the Mac from fixtures.

**By hand (about 20 minutes):** the herdr items and the plain Windows Terminal items
above; a session started from a pwsh shortcut with Windows Terminal as the default
terminal (defterm) falls back to `app-activate`; two Windows Terminal windows each
running `herdr` attached (the log says `app` confidence); `--dump-desktop` with herdr and
those sessions running, attached, plus the `HERDR_SOCKET_PATH` value as the brain saw it
(user name masked). VS Code only if it is installed; otherwise its case rests on fixtures.

#### W3 status (developer, 2026-10-29): built; the locator's Windows tests green on the Mac; the PC checks open

Found on the maintainer's PC with the 1.2.0 test folder (before W3): a click on a herdr
session's sheep ran herdr's CLI (the pane was selected, `Focused("herdr (wH:t5, pane
wH:p5)")`) but nothing raised Windows Terminal, the walk had no process list (`ps failed
(NotFound)`), each herdr run flashed a console window, and the tray icon (a macOS template,
drawn black) vanished on a dark taskbar. The last two are fixed in PR #4 (`CREATE_NO_WINDOW`
for the locator's commands; a white tray sheep with a dark outline off macOS).

Built:

- **The `Native` seam** (`locator.rs`; real: `winlocate.rs`): the process list (Toolhelp;
  "this user's" by the token's SID, for `claude.exe`, `node.exe` and `herdr.exe` only),
  a process's marker variables and its command line read from its PEB
  (`NtQueryInformationProcess` + `ReadProcessMemory`, same user only; only the asked-for
  keys are kept: `env_values`), and raising a window (`SW_RESTORE` if minimised, then
  `SetForegroundWindow`; if that is refused, `FlashWindowEx` and the log says `foreground
  refused for <exe>`). `Locator::with_native` switches to it and drops `iterm2` and
  `terminal-app` with one line each. macOS is unchanged (`ps`, `open`, AppleScript).
- **The Windows window list** (`usable_windows`, shared with the tests): `EnumWindows`,
  not cloaked, not tool windows, not ours, at least 50 px each way unless minimised;
  owner = exe name, bounds = DWM's in physical px, with the handle. A minimised window can
  be raised but never places a sheep.
- **The walk**: as macOS, up to the first window owner, but on Windows `explorer.exe`
  never counts (Windows Terminal's parent; it owns File Explorer's windows). A session
  whose walk reaches it (a shortcut handed to Windows Terminal, defterm) falls to
  `app-activate`.
- **herdr**: settled from the PC's W1 dump: herdr's server is a child of the attached
  client (`herdr.exe` → `herdr.exe` → `pwsh.exe` → `WindowsTerminal.exe`), and every
  `claude.exe` is under the server. A client is a `herdr.exe` with no `herdr.exe` above it.
  Several clients: `app` confidence. A click runs herdr's CLI (workspace, tab, agent
  focus) and raises the client's Windows Terminal window; with no attached client the log
  says `no attached herdr client has a window (detached?)`. `HERDR_SOCKET_PATH` on Windows
  must be `\\.\pipe\<plain name>` or an existing absolute file (`check_socket_windows`); the
  pipe's server isn't asked who it is (connecting would take one of herdr's pipe
  instances). The herdr path: on PATH, then `%LOCALAPPDATA%\Programs\Herdr\bin\herdr.exe`.
- **process-tree**: `claude.exe`, or `node.exe` whose command line has `claude-code`, of
  this user, whose `WT_SESSION` (and `TMUX_PANE`) match; exactly one, or no answer. A
  session with `HERDR_PANE_ID` never matches here (its `WT_SESSION` is the server's).
- **app-activate**: `claude-desktop` → `claude.exe`, `vscode` → `Code.exe`, `WezTerm` →
  `wezterm-gui.exe`, a `WT_SESSION` → `WindowsTerminal.exe`; the exe's frontmost window.
- **Windows wording**: the default chain `herdr > vscode > process-tree > app-activate`,
  `locator: … (herdr at …; no permission needed)`, `dock: none on Windows …`; doctor's
  "appears above the taskbar" and its herdr CLI line without iTerm2/Terminal.
- **The black flash on the first hover** (PC test, 2026-10-29: the strip went black for a
  moment the first time the cursor came near a sheep): tao's click-through toggle drops
  `WS_EX_LAYERED` along with `WS_EX_TRANSPARENT`, and a see-through window that stops
  being layered repaints black. The strip's style keeper (`win.rs`, `kept_style`) now keeps
  `WS_EX_LAYERED` for good, so only `WS_EX_TRANSPARENT` toggles. PC check: no flash on
  hover, and a click on a sheep still lands.

Tested on the Mac: 19 Windows locator tests over `test/fixtures/windows/desktop-pc-herdr.json`
(the PC's W1 dump, trimmed to the terminal's, herdr's and claude's processes and windows)
with a fake `Native`: herdr exact on the PC's Windows Terminal window and a click raising it
with no macOS command run; herdr detached; two clients; process-tree by `WT_SESSION`; two
Windows Terminal windows (`app`); two lookalikes (not guessed); a walk reaching explorer;
a minimised window and a refused foreground; `node.exe`; another user's `claude.exe`;
environment-block parsing; socket names. `npm run test:cargo` 344 passed;
`cargo xwin check --workspace --target x86_64-pc-windows-msvc` no warnings; `npm test` 1061
passed, 1 skipped. Not run on Windows yet: the real `winlocate.rs` (PEB offsets for x64 and
ARM64, the token check, the foreground rule for a click on a no-activate window).

Open: the by-hand list above, on the PC, with this branch's CI test folder.

### W4. Dragging a sheep on Windows, and M12's two-monitor check

Builds:

- `mouse_down()` with `GetAsyncKeyState`;
- the drag's cursor mapping in the `Physical` space;
- M12 step 1 (a), (b), (d) and (e) re-run on Windows, answers in W4's status.

**Done when:** M12's Done-when items hold on Windows, checked with M12's by-hand list
1–9 on the maintainer's PC, with these Windows notes:

- check 5 (two monitors) with the monitors at different scales: the carried sheep is
  drawn on the other strip within 100 ms, at the cursor (not offset by the scale ratio);
- check 6 (the fail-safe): after the reload, the click under the strip reaches a desktop
  icon;
- W4's status records that M12 check 5 is now proven for Windows and still open for macOS
  (WD11).

**By hand (about 20 minutes):** M12's list, with a mouse and a touchpad.

### W5. Screen sharing and full screen

Builds:

- the Windows process-name reader and Windows rules in `sharing_warnings_for`;
- the full-screen detector per monitor and `SHQueryUserNotificationState` (WD5);
- the effective `privacy.captureExclusion` in doctor, and `GetWindowDisplayAffinity` read
  back when it is on (the per-OS default itself, false on Windows, its log line and
  `config.example.json` without the key came forward to W1: deviation W1-10);
- the README notes that the default differs by OS (when the README covers Windows).

**Done when:**

- with the defaults: Win+Shift+S over the strip shows the sheep (first checked in W1, step
  8; here again with the W5 build);
- Zoom sharing the whole screen: `CptHost.exe` hides the strip at once; it is back 2 s after
  sharing stops; the far side never sees it once sharing has started (the hide lands
  within one 500 ms poll; recorded in W5's status);
- the Teams check is recorded: whether any process runs only while the new Teams shares.
  If one does, it joins the `win32` default list and Teams sharing hides the strip; if
  not, the README says to use *Hide everything* before sharing in Teams, as for Meet;
- a full-screen video on monitor 2 hides monitor 2's strip only, back 2 s after leaving
  full screen; a PowerPoint slideshow hides all strips;
- `privacy.hideWhenFullscreen: false` turns the detector off; `privacy.hideWhenSharing:
  false` turns the process check off;
- opt-in: `privacy.captureExclusion: true` and `bellwether shell restart`: the sheep are
  absent from Win+Shift+S and from a Zoom share's far side, including while the cursor is
  over a sheep (click-through toggling), with no black rectangle; doctor shows it on.

**By hand (about 20 minutes):** a Zoom call and a Teams call with a second device (a phone
in the meeting); Meet in a browser with *Hide everything*; the full-screen cases; the
opt-in check.

### W6. Start at login, the brain's life, single instance

Builds:

- the Run values, the `StartupApproved` reader, `autostart` on Windows, its registry
  allowlist (WD6);
- the app's supervision of the brain, the detached start, the version check and restart;
- `ms-settings:startupapps` from `autostart settings` and doctor.

**Done when:**

- `bellwether autostart install --write`, sign out and in: the brain answers `/health`
  and the strip shows (test port);
- turning "Bellwether Brain" off in Task Manager makes `autostart status` and doctor say
  so, with the way to turn it back on;
- ending the brain in Task Manager while the strip runs: a new brain answers within 15 s
  and the log says why;
- a second launch of the app doesn't start a second strip;
- a brain of another version (an older test zip) is replaced by this one's on app start;
- `autostart uninstall --write` removes both values and nothing else (registry export
  before and after, diffed).

**By hand (about 15 minutes):** the items above.

### W7. The setup window and the tray on Windows

Builds:

- `setup_status` hiding the macOS-only steps; the start-at-login and CLI-tool steps on
  Windows; `rfd::FileDialog` for Import…;
- the PATH change with its preview, `WM_SETTINGCHANGE`, and its undo;
- the Windows tray icon (`.ico`, light and dark), the overflow hint;
- the capability tests unchanged (still 7 strip commands, 11 setup commands).

**Done when:**

- the setup window on first launch walks through start at login, hooks (against a
  scratch settings file in the test build), pet import with the Windows dialog, the CLI
  step, and Done with doctor's results;
- a new terminal after the CLI step finds `bellwether`;
- the tray menu's items all work, and the icon is readable on light and dark taskbars.

**By hand (about 15 minutes):** the items above, in light and dark mode.

### W8. The installer and the updater (unsigned)

Builds:

- the Windows overlay (`tauri.windows.dist.conf.json`): NSIS, `currentUser`,
  `installerHooks`, `createUpdaterArtifacts`, `installMode: passive`;
- `scripts/build-windows.mjs` (builds both executables, places `bin\bellwether.exe`,
  calls `tauri build`, with `--sign` reserved for W9);
- `release.mjs assets --windows` and `release.mjs merge`;
- the update flow with the brain stop (WD9), and the `rfd` prompt;
- CI: the unsigned NSIS build, release files with a throwaway key, the merged
  `latest.json` checked for all three platform keys.

**Done when:**

- the installer runs without admin, installs to the recorded directory, adds a Start
  menu entry, and the app starts;
- uninstalling removes Bellwether's hooks (with a backup), both Run values and the PATH
  entry, and leaves other tools' hooks byte for byte;
- an update from version N to N+1 through a local feed (as RELEASING.md's "Testing the
  updater locally"): the prompt, a passive install, the app back, the brain on N+1,
  `state.json` kept, and at most a few seconds of hook errors in a running session
  (measured and recorded);
- an update while `bellwether status --watch` runs in a terminal doesn't fail the install
  (the console CLI is not under `Bellwether.exe`'s path, but it is under the install
  directory; the result is recorded).

**By hand (about 25 minutes):** install, use, update, uninstall, in that order, on the
test build's identifier.

### W9. Signing and the release pipeline

Builds:

- `scripts/sign-windows.mjs` and `signCommand` in the overlay;
- `build-windows` in release.yml and the merged `publish` (WD9);
- RELEASING.md's Windows part: one-time setup (Auth and permissions), each release, the
  checks.

**Done when:**

- a `v*` tag on a test pre-release produces one draft with the DMG, the macOS updater
  files, the signed `-setup.exe` and its `.sig`, one `latest.json` with three platform
  keys, and one `SHA256SUMS`;
- `Get-AuthenticodeSignature` says `Valid` for `Bellwether.exe`, `bin\bellwether.exe`, the
  uninstaller and `-setup.exe`, with a timestamp;
- no signing ID or secret appears in the repository or the log;
- the macOS job's steps are unchanged.

**By hand (about 10 minutes):** download the signed installer in Edge on the PC; record
what SmartScreen and (if on) Smart App Control show; install; an installed copy updates
itself from the draft's feed (published as a pre-release).

### W10. winget, docs, and the release with Windows

Builds:

- `winget.yml` and the manifest template under `packaging/winget/`;
- README: "Install on Windows", "First launch" differences, Windows paths, screen sharing
  on Windows (the process list, *Hide everything*, and `privacy.captureExclusion` as an
  opt-in that also hides the sheep from your own screenshots), herdr on Windows,
  uninstall; DEVELOPMENT.md: Windows CI, the test zip,
  `--dump-desktop`; CHANGELOG entry;
- the release (1.2.0, Q13).

**Done when:**

- `winget install Fanzoo.Bellwether` installs the signed release on the maintainer's PC
  (or the manifest PR passes winget's validation, if not yet merged);
- the whole by-hand list of W1–W9 passes once on the signed, installed build, with the
  maintainer's real settings file (his first real install);
- the README's Windows section matches what he saw.

**By hand (about 30 minutes):** the full pass above.

## Test plan

Per milestone, the tester verifies the Done-when items: CI results for the automated
ones, the maintainer's notes, logs and screenshots for the by-hand ones. Specifically:

- **W1:** the Windows CI job; the pure geometry tests on both OSes; screenshots and `winver`; the heartbeat
  lines in `shell.log`; the attached `desktop.json`.
- **W2:** Windows and macOS CI; `icacls` output; a real session's lines in `brain.log`
  (event names only, never bubble text); the CRLF and BOM vectors.
- **W3:** the fixture tests (herdr client/server trees included); the log's strategy
  lines from the PC, `herdr (…)` among them.
- **W4:** M12's checks with Windows notes; the cursor-mapping tests with mixed scales.
- **W5:** the meeting checks (what the far side saw); the full-screen log lines; a
  screenshot with the defaults (sheep present) and with the opt-in (sheep absent).
- **W6:** the registry export diff; the supervision log lines; doctor's output when the
  item is turned off.
- **W7:** the capability tests unchanged; the PATH diff before and after.
- **W8:** the CI release-file check (three keys in `latest.json`); the update gap measured.
- **W9:** the draft's file list; `Get-AuthenticodeSignature` output; a search of the log
  for the signing IDs (none).
- **W10:** the winget validation result; the full by-hand pass.

**Touching a live machine without changing it.** The maintainer's Windows PC is his own.
Until W10:

- by-hand checks use the test zip or the test build's identifier (`local.bellwether`), a
  test port and a scratch home (WD12);
- the test build refuses `hooks install` without `--settings <file>`, refuses to write Run
  values outside W6's check (where they are removed again with `autostart uninstall
  --write`, diffed by registry export), and never touches `%APPDATA%\Bellwether`;
- real sessions use `claude --settings <file>`, so `%USERPROFILE%\.claude\settings.json`
  never changes.

**Fixtures and offline modes the developer must provide:** `--demo` (exists), the test
zip, `--dump-desktop`, `test/fixtures/windows/desktop-*.json`, fakes for `Processes` and
`Windows`, `StartupApproved` byte fixtures, installer vectors with CRLF and BOM, and a
local updater feed (as RELEASING.md describes for macOS).

## Risks and open questions

### Risks

- **WR1 WebView2 throttles the strip.** `background_throttling` is unsupported on Windows
  [20]. Mitigation: W1 measures; the browser-arguments fallback (WD2). If both fail, a
  pending Web Lock in the page (wry's own suggestion) [20].
- **WR2 Click-through flicker, or a black capture with the opt-in.** Toggling
  `WS_EX_LAYERED` on a WebView2 window [18]. Mitigation: W1 and W5 look for it; keep
  `WS_EX_LAYERED` on and toggle only `WS_EX_TRANSPARENT`.
- **WR2b Screen-share hiding is weaker than first planned.** With exclusion off by
  default, a share hides the strip only when a known sharing process runs (Zoom) or a
  full-screen app covers the monitor. Teams and browser meetings may show the sheep, and
  the first frame of a Zoom share may too (the 500 ms poll). Mitigation: the README and
  the tray's *Hide everything*; the opt-in for users who want the strong layer.
- **WR14 herdr on Windows.** Native Windows herdr is recent [22], its pipe name and its
  server process's shape are undocumented, and its CLI could change. Mitigation: W3
  records the real values; the strategy fails soft to `app-activate` (the Windows Terminal
  window), as the macOS strategy does when herdr is missing or slow.
- **WR3 The foreground is refused.** Windows' foreground rules [7] may treat a click on a
  no-activate window as not ours. Mitigation: W3 proves it; the flash fallback.
- **WR4 Reading another process's environment** relies on the PEB layout, which is
  undocumented in parts. Mitigation: the `sysinfo` fallback; markers are a refinement, and
  the parent walk works without them.
- **WR5 Mixed DPI.** Every position must be in one space. Mitigation: WD3's seam and
  table tests; W1 and W4 by hand at 100% and 150%.
- **WR6 Virtual desktops.** tao has no all-workspaces support on Windows [20]. Mitigation:
  W1 records; `IVirtualDesktopManager` fallback.
- **WR7 Signing eligibility and SmartScreen.** Artifact Signing's identity validation may
  take long or fail [8]; a new identity starts with no reputation [2][16]. Mitigation: start
  validation now (decided, Q1); the OV route as the fallback; W9 records what users see.
- **WR8 The update gap.** Several seconds of hook errors per update (WD9). Mitigation:
  accepted (Q7, decided); the fallback is running the brain from a versioned copy
  under `%LOCALAPPDATA%\Bellwether\brain\<version>\`, so the installer never waits for it.
- **WR9 The `StartupApproved` layout** is community knowledge [17]. Mitigation: unknown
  bytes read as `unknown`, never as off; no writes.
- **WR10 The taskbar moves** (Windows 11 Insider builds, mid-2026 [14]). Mitigation: the
  work-area rule follows any edge without a taskbar API.
- **WR11 Shared code regresses macOS.** Mitigation: macOS CI unchanged and required; every
  Windows change behind a seam; the macOS by-hand list from M11 run once before the 1.2.0
  release.
- **WR12 CI desktop.** Hosted Windows runners may have no interactive desktop for the
  strip smoke test. Mitigation: the step reports a skip; the maintainer's by-hand checks
  cover it.
- **WR13 Claude Code changes on Windows.** HTTP hooks avoid the shell issues, but
  paths and policy locations have moved before (the managed-settings path [10]).
  Mitigation: doctor's `--e2e` check in the release checklist, on Windows too.

**Assumptions** (to confirm):

- WA1. A shown taskbar's space is excluded from each monitor's work area, secondary
  taskbars included (W1 check 2).
- WA2. Topmost tool windows show on every virtual desktop (W1 check 6).
- WA3. Windows Terminal is the parent of its shells, so a `herdr.exe` client's chain
  reaches `WindowsTerminal.exe`; herdr's server is told apart from its clients by its
  parent chain or a `server` argument; `HERDR_SOCKET_PATH` is a named-pipe path on
  Windows (W3).
- WA4. A click on a no-activate strip allows `SetForegroundWindow` (W3).
- WA5. `tauri build` doesn't bundle the extra `[[bin]]` at the top level (W8).
- WA6. Fanzoo Technology, Inc. completes Artifact Signing's organisation validation
  (started 2026-10-20) before W9.
- WA7. The maintainer's PC (Windows 11, build recorded in W1) runs Claude Code inside
  herdr inside Windows Terminal, with two monitors, one of which he can set to 150%.

### Open questions for the maintainer

All answered on 2026-10-20. Each question keeps its number, so references such as "(Q8)"
elsewhere still point here.

### Decisions (maintainer, 2026-10-20)

1. **Signing route and budget.** Decided: Azure Artifact Signing (about USD 9.99 a month),
   added to Fanzoo's existing Azure account. Identity validation for Fanzoo Technology,
   Inc. starts now (steps in Auth and permissions). Account, subscription, tenant and
   profile IDs stay out of the repository.
2. **Windows versions.** Decided as assumed: Windows 11 (23H2 and later) supported;
   Windows 10 22H2 best effort, not tested by hand. The test PC runs Windows 11; its exact
   build is recorded with `winver` in W1's first by-hand step.
3. **Architectures.** Decided as assumed: x64 only; ARM64 PCs run it under emulation.
4. **Installer type.** Decided as assumed: NSIS, per user, no admin, no MSI.
5. **Channels.** Decided as assumed: GitHub Releases and winget; Scoop and the Microsoft
   Store later, if ever.
6. **One tag, both platforms.** Decided as assumed: once Windows ships, a Windows build
   failure blocks the whole draft release.
7. **Update gap.** Decided as assumed: several seconds of hook errors during a Windows
   update are acceptable; WR8's fallback only if it proves annoying.
8. **The uninstaller edits settings.json.** Decided: yes. Uninstalling removes
   Bellwether's hooks (with a backup) and its login entries (and the PATH entry).
9. **Capture exclusion by default.** Decided: **no.** The maintainer wants the sheep in his
   own screenshots. On Windows `privacy.captureExclusion` defaults to false and is an
   opt-in; screen-sharing hiding relies on the process list and the full-screen detector,
   as on macOS (WD5, W5). macOS keeps its default.
10. **Full screen.** Decided as assumed: a monitor's strip hides while a window covers that
    monitor (`privacy.hideWhenFullscreen`, default true).
11. **WSL.** Decided as assumed: Claude Code inside WSL is out of the first Windows
    release.
12. **Brain restarts.** Decided as assumed: Run values plus the app restarting a crashed
    brain, not Task Scheduler.
13. **Version.** Decided as assumed: Windows ships in 1.2.0 for both platforms, after
    `v1.2.0-rc.N` pre-releases.
14. **The test PC.** Answered: Windows 11 (build to be recorded in W1). Claude Code runs in
    **herdr inside Windows Terminal**; no VS Code, so the VS Code path is secondary
    (fixtures and unit tests). One monitor can be set to 150% for the checks. Consequences:
    the `herdr` strategy is built for Windows and is first in the chain (WD4, W3), and the
    real-session checks in W2 and W3 run inside herdr.
15. **Click precision.** Decided as assumed: window level for Windows Terminal. Inside
    herdr, herdr's own workspace, tab and pane are selected through its CLI (WD4); the
    Windows Terminal tab hosting the herdr client is not.
16. **Data directory.** Decided as assumed: data in `%APPDATA%\Bellwether`, logs in
    `%LOCALAPPDATA%\Bellwether\logs`.

---

## Sources

1. Tauri, Updater plugin (v2): platform keys, Windows artifacts, `installMode`, exit on
   install. https://v2.tauri.app/plugin/updater/
2. Tauri, Windows code signing: OV and EV, SmartScreen since 2024, `signCommand`, Azure
   Key Vault, Artifact Signing with `artifact-signing-cli`.
   https://v2.tauri.app/distribute/sign/windows/
3. Tauri, Windows installer: NSIS install modes, `%LOCALAPPDATA%`, WebView2 install modes,
   installer hooks, MSI needs Windows. https://v2.tauri.app/distribute/windows-installer/
4. Microsoft, SetWindowDisplayAffinity: `WDA_EXCLUDEFROMCAPTURE` since Windows 10 2004,
   DWM only, not a security guarantee.
   https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowdisplayaffinity
5. Microsoft, SHAppBarMessage; use GetMonitorInfo for the working area.
   https://learn.microsoft.com/en-us/windows/desktop/api/Shellapi/nf-shellapi-shappbarmessage
6. Microsoft, ABM_GETAUTOHIDEBAR (primary taskbar's monitor only; use the EX form).
   https://learn.microsoft.com/en-us/windows/desktop/shell/abm-getautohidebar
7. Microsoft, SetForegroundWindow: the conditions, including "received the last input
   event". https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setforegroundwindow
8. Microsoft, Artifact Signing pricing (Basic USD 9.99/month, 5 000 signatures) and
   quickstart (public trust eligibility).
   https://azure.microsoft.com/en-us/pricing/details/artifact-signing/ ,
   https://learn.microsoft.com/azure/artifact-signing/quickstart
9. Anthropic, Claude Code settings: `~/.claude` is `%USERPROFILE%\.claude` on Windows;
   `CLAUDE_CONFIG_DIR`. https://code.claude.com/docs/en/settings
10. Anthropic, Claude Code managed settings: `C:\Program Files\ClaudeCode\`, HKLM and HKCU
    `SOFTWARE\Policies\ClaudeCode`, the legacy `ProgramData` path no longer read.
    https://code.claude.com/docs/en/managed-settings
11. Anthropic, Claude Code hooks: HTTP hooks, header interpolation, `allowedEnvVars`;
    command hooks on Windows run in Git Bash or PowerShell.
    https://code.claude.com/docs/en/hooks
12. Microsoft, Windows Terminal command-line arguments (`-w`, `focus-tab`).
    https://learn.microsoft.com/windows/terminal/command-line-arguments
13. Microsoft, Submit your manifest to the winget repository.
    https://learn.microsoft.com/windows/package-manager/package/repository
14. PCWorld, Windows 11's movable taskbar in a preview build (2026).
    https://www.pcworld.com/article/3205037/windows-11s-most-requested-feature-just-showed-up-in-a-preview-build.html
15. PC-Welt, Windows 10 consumer updates extended to 12 October 2027 (not confirmed on a
    Microsoft page). https://www.pcwelt.de/article/3177497/windows-10-bekommt-ein-weiteres-jahr-lang-updates-sensation.html
16. Computerworld, Windows 11 Smart App Control explained; Microsoft Q&A on signed apps
    blocked by Smart App Control.
    https://www.computerworld.com/article/4043925/windows-11-smart-app-control-explained.html ,
    https://learn.microsoft.com/en-us/answers/a/12641816
17. PowerShell.org forum, StartupApproved values (02 on, 03 off; community knowledge).
    https://forums.powershell.org/t/enable-disable-startup-programs-in-windows-10/8537
18. Electron, content protection fix (`WS_EX_LAYERED` and a black window after hide and
    show). https://ayakael.net/mirrors/electron/commit/95f81f598071134f8914f9201923bd051c682ffd
19. Chromium, auto-hide taskbar detection note (`ABM_GETAUTOHIDEBAREX` unreliable with a
    monitor rectangle). https://crrev.com/c64c903e110dc5f34a6348eb9b83e15900d96fbf
20. Sources read locally from `~/.cargo/registry` for the versions in `Cargo.lock`:
    `tao` 0.37.1 (`platform_impl/windows/window.rs`, `window_state.rs`, `dpi.rs`, `util.rs`),
    `wry` 0.57.0 (`lib.rs` background throttling, `webview2/mod.rs` default browser
    arguments), `tauri` 2.12.1 (`webview_window.rs`, `additional_browser_args`),
    `tauri-plugin-updater` 2.13.2 (`updater.rs`: `get_urls`, NSIS `/UPDATE`, exit),
    `tauri-plugin-single-instance` 2.5.2 (`platform_impl/windows.rs`); and Tauri's NSIS
    `utils.nsh` (`CheckIfAppIsRunning`, Restart Manager, force-close in passive mode).
    https://github.com/tauri-apps/tauri/blob/dev/crates/tauri-bundler/src/bundle/windows/nsis/utils.nsh
21. crates.io, checked 2026-10-19: `tauri` 2.12.4, `tauri-plugin-updater` 2.14.0, `rfd`
    0.17.2, `windows-sys` 0.61.2, `sysinfo` 0.39.6. https://crates.io
22. herdr docs, Windows (docs 0.9.5: native Windows generally available on ConPTY,
    background server, attach from Windows Terminal, install layout) and agent automation
    (`HERDR_PANE_ID`, `pane focus` / `agent focus`).
    https://herdr.dev/docs/windows-beta/ , https://herdr.dev/docs/agent-automation/
23. herdr docs, Socket API (socket resolution, a named pipe on Windows, the variables
    injected into panes, `pane.get`, `workspace.focus`, `tab.focus`, `agent.focus`).
    https://herdr.dev/docs/socket-api/

## Deviations

Recorded by the developer while building W1–W10, in the macOS plan's form. Each one: what
the plan said, what is true, and what was done.

1. **Deviations in building milestone W1** (developer, 2026-10-19). Numbered W1-n, as the code cites them.

- **W1-1, `icons/icon.ico`.** The plan expected the shell crate to need only `cfg(windows)`
  stubs. `tauri-build` also refuses to build for Windows without `icons/icon.ico` (the
  exe's resource icon), so the whole-workspace check could never have passed. Made from
  `icons/icon.png` with the pinned CLI's `tauri icon` (only the `.ico` kept). The
  notification-area icon is still the macOS template sheep, drawn in black, and may be
  hard to see on a dark taskbar; W7's colour `.ico` is separate.
- **W1-2, the tool-window style is kept, not set once.** WD2 says to add
  `WS_EX_TOOLWINDOW` through the HWND after the window is built. tao 0.37.1 rewrites the
  whole extended style (`SetWindowLongW(GWL_EXSTYLE, …)` from its own flags) whenever a
  window flag changes: click-through on and off, show, hide (`window_state.rs`,
  `apply_diff`). A style set once would be gone at the first toggle. `win.rs` installs a
  window subclass that adds `WS_EX_TOOLWINDOW` (and removes `WS_EX_APPWINDOW`) in every
  `WM_STYLECHANGING` for the extended style, then sets it once.
- **W1-3, the test folder's port and home are set by `run-test.cmd`, not compiled in.**
  WD12 5 describes a build "with a compile-time test port and scratch home". In W1 the
  folder runs only the demo, which starts no brain and reaches no CLI command
  (`run-test.cmd` always passes `--demo` first, which dispatch.rs gives to the app). The exe
  runs in the repository layout: `run-test.cmd` sets `BELLWETHER_AGENT_DIR` to the folder
  (its `config.json`, port 47491, and `pets\`) and `BELLWETHER_HOME` to `home\` beside it,
  and clears the other `BELLWETHER_*` variables. Started any other way (a double-click on
  the exe) it finds no checkout and exits. The compile-time port and home, and the refusal
  of `hooks install` without `--settings`, come with W2, which puts the brain and the CLI
  in the folder. The `--dump-desktop` switch for a release-profile exe is a new cargo
  feature, `test-build`, which only `scripts/build-windows-test.mjs` turns on (tested).
  The plan names the CI artifact `bellwether-windows-test.zip`; the job uploads the folder
  and GitHub serves it as that zip.
- **W1-4, the desktop file's shape.** The data model's `desktop.json` used the key
  `windows` twice (the OS version and the window list). The version is under `os`
  (`version` from the registry's `CurrentMajorVersionNumber.Minor.CurrentBuildNumber.UBR`,
  `displayVersion` such as `24H2`). Rectangles are `[left, top, right, bottom]` in
  physical pixels. Each monitor also has `strip` (where the strip would stand), and each
  window `topmost`. The herdr subcommand word for `herdr.exe` processes is not in W1's
  file: it needs the process's command line, which the W3 code reads (PEB); W3 adds it.
- **W1-5, `--demo-monitors`.** `--demo` showed its flock on the primary monitor only, and
  W1's Done-when wants it on every monitor. A new flag (with `--demo`) opens a strip on
  every monitor and gives each the whole demo flock; `run-test.cmd` passes it. `--demo`
  alone is unchanged, on macOS too.
- **W1-6, the heartbeat is its own flag.** The page already had a heartbeat, but only
  under `BELLWETHER_SHELL_TRACE` (a line a second, among every other trace line).
  `--heartbeat` logs one line every 10 s per strip with what a throttled page would show
  (ticks that arrived, the longest gap), and the shell itself notices and logs a silence.
- **W1-7, the cargo tests on Windows.** W1's Done-when has `npm run test:cargo` passing on
  Windows. Tests that can't run there are compiled out with `#[cfg(unix)]` and a comment
  each: the launchctl stand-ins and every test using them (agent.rs, brain.rs), symlinks
  (migration.rs, onboarding.rs's CLI link, tester_m7.rs), a FIFO (settings.rs), Unix
  permission bits (tester_m7.rs), `/bin/sleep` and `/bin/echo` (legacy.rs), `/usr/bin/true`
  (toapp.rs), and the integration files about launchd, LaunchAgents and migrate
  (brain_version.rs, migration_fixtures.rs, no_launchagents.rs), four more toapp.rs tests
  found by the VM run (W1-11), and the development CLI's
  home rule (dev_home.rs, until W2 brings the CLI to Windows). On the Mac they all still
  run. This was a static pass: the first CI run may find more, so the job runs with
  `--no-fail-fast`. The job checks out with LF line endings (`core.autocrlf false`):
  several tests read their own source, and the fixtures and pinned pets compare bytes.
- **W1-8, `sys.rs`.** The shell called `libc::getuid`, `getppid` and `kill` outside any
  `cfg`, which doesn't compile for Windows. They go through `sys::uid`, `parent_pid`,
  `alive` and `terminate`, exactly the libc calls on Unix. On Windows they are stand-ins
  until the milestone that needs them (uid 0 until W3's SID check; no parent pid and no
  liveness until W2; no stop until W6's tokened stop), so `shell stop` and `autostart
  uninstall` can't stop a strip on Windows yet. Neither is usable there before W6.
- **W1-9, compiling here.** The plan says the Mac has the Windows target installed. It
  does, but `cargo check --target x86_64-pc-windows-msvc` of the whole workspace stops at
  `ring`'s C build (no Windows headers; CI's existing cross-check covers only core and
  brain). cargo-xwin and Homebrew's LLVM and LLD were installed on the Mac to check and
  link (docs/DEVELOPMENT.md, "Windows (in progress)").
- **W1-11, no exe name with "install", "setup", "update" or "patch"** (the VM run,
  2026-10-20). Windows' installer detection asks for elevation before it runs an exe whose
  name contains one of those words, so `crates/core/tests/installer.rs` (an exe,
  `installer-<hash>.exe`, like every test target there) never ran: "The requested
  operation requires elevation (os error 740)". Renamed `crates/core/tests/hook_vectors.rs`
  (references in test/hooks.test.mjs and test/DIVERGENCES.md updated; the history in
  DISTRIBUTION-PLAN.md keeps the old name). **Naming rule from now on:** no cargo test
  target, binary or package name may contain `install`, `setup`, `update` or `patch` (any
  case); `test/windows.test.mjs` fails if one does. Shipped installers (Tauri's
  `-setup.exe`) are meant to ask, and are not test targets. The same run found five more
  failures, handled so:
  - `onboarding::tests::the_preview_is_the_clis_diff_masked_and_install_applies_exactly_it`
    asserted the backup path contains `backups/settings-`; on Windows it is
    `backups\settings-…`, which is right. The assertion now checks the path's parts (its
    folder is `backups`, its name starts `settings-`) on every OS. Fixed, not skipped.
  - Four `toapp` tests (`a_run_killed_between_the_plist_moves_is_recovered_by_a_rerun_or_an_undo`,
    `one_migration_at_a_time`, `undo_quits_the_strip_before_the_old_agents_start`,
    `the_swap_marker_is_there_only_during_step_4`, and their helper `AppIsMe`) are
    `#[cfg(unix)]`: `migrate --to-app` moves LaunchAgents plists and takes an `flock`,
    macOS only, and is never reached on Windows (as W1-7).
- **W1-12, the re-fit after a scale change is deferred** (BR-3K's W1 verdict, finding 1).
  The `ScaleFactorChanged` handler called `run_on_main_thread`, which in
  tauri-runtime-wry 2.12.1 runs the closure at once when called on the main thread
  (`send_user_message`). The handler runs inside tao's `WM_DPICHANGED` handling, whose
  `SetWindowPos` (Windows' suggested rectangle) comes after it, so the re-fit was
  overwritten and only the 2 s re-read fixed the strip, while the log said "re-fitting".
  Tauri's event gives the new size by value, so it can't be written back. Now the handler
  posts the re-fit from another thread, 50 ms later, which queues it for a later turn of
  the event loop. The log says what happens: `strip-1: scale changed to 1.25; Windows
  resizes it to <w>x<h>; it is put back to its frame after that`, then `strip-1: moved
  after a scale change: x … physical px at scale 1.25` when the frame differed. The pure
  part, `PhysRect::matches` (what the re-fit and the 2 s re-read compare), has a test
  with Windows' kind of suggestion. Whether the deferred re-fit beats the 2 s re-read on a
  real scale change is by-hand step 4; the re-read stays as the backstop.
- **W1-13, a failed `--dump-desktop` says why** (BR-3K finding 2).
  `run-test.cmd --dump-desktop Z:\nope\x.json` said "Could not write" and exited 0, and
  nothing said why: `start /wait` on a GUI exe loses its stderr. The exe now logs every
  outcome of `--dump-desktop` to shell.log (`--dump-desktop: cannot write …: <error>`, or
  `--dump-desktop: wrote …`), and run-test.cmd exits with the exe's code (1 if the file
  is missing) and prints the log's `--dump-desktop:` lines.
- **W1-14, a fresh log per run** (BR-3K's observation). shell.log appends across runs, so
  an attached log held old runs too. Each `run-test.cmd` demo run now moves the old log to
  `home\shell.previous.log` first; the folder's README says so. A dump adds its lines to
  the current log.
- **W1-15, the strips are put back on top, not set once** (the maintainer's PC run,
  2026-10-21). WD2 relied on `always_on_top(true)` (`HWND_TOPMOST`) at creation. On his PC
  the sheep were once behind ordinary windows; he couldn't reproduce it with Win+D or a
  lock. Windows can drop a window's topmost state or put ordinary windows over it when the
  taskbar or Start is activated, a full-screen app runs or explorer restarts, and nothing
  put it back. Now `strips::keep_on_top` puts every strip back with `SetWindowPos(HWND_TOPMOST,
  SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE)` (never activating it or moving focus; the
  other extended-style bits are untouched) on the 2 s re-read and on every foreground
  change (`SetWinEventHook(EVENT_SYSTEM_FOREGROUND)`, out of context, skipping our own
  process, on the main thread: `win::on_foreground_change`). Before it does, `win::keep_topmost`
  reads whether `WS_EX_TOPMOST` was gone; the verdict is pure (`desktop::on_top`, tested on
  the Mac). (A z-order walk for an ordinary window over the strip was dropped in W2-17.) When the put-back was needed, shell.log gets one
  line naming the strip and the trigger (`strip-0: was no longer on top (WS_EX_TOPMOST
  gone, seen on a foreground change); put back on top`, or `… could not be put back on
  top …`), at most one a minute per strip, the next
  line counting the ones not logged (`desktop::OnTopLog`, tested). The start-up log says
  `strips: put back on top every 2 s and on every foreground change …` (or a warning if the
  hook couldn't be set). Windows only; macOS unchanged. A strip put back on top also goes
  above other topmost windows (another app's always-on-top overlay); it is click-through
  except over a sheep. Proven in the VM by dropping topmost by hand (W1 status); whether it
  cures what the maintainer saw is by-hand check 14.
- **W1-10, capture exclusion off by default on Windows, brought forward from W5**
  (maintainer's run, 2026-10-20). The plan put the per-OS default of
  `privacy.captureExclusion` in W5, but W1's by-hand step 8 expects the sheep in
  Win+Shift+S captures. The maintainer ran the W1 test folder in a Windows 11 ARM64
  Parallels VM (the x64 build under emulation): placement, drag, clicks and the heartbeat
  looked right, but the sheep were missing from screenshots, with `strip-0: capture
  exclusion on (sharingType none)` in the log. Two causes: the shell defaulted the key to
  true on every OS, and `config.example.json`, which the test folder's `config.json` is
  made from, set it to `true` explicitly. Now `privacy::capture_exclusion` (pure, tested on
  the Mac) gives config.json's boolean when there is one, on any OS, and otherwise the OS's
  default: false on Windows, true on macOS (unchanged) and elsewhere. The key is gone from
  `config.example.json` so each OS gets its default (macOS behaviour is the same: absent
  means true there). The log line says the value and why: `privacy: capture exclusion off
  (the Windows default; privacy.captureExclusion: true turns it on), hide while sharing …`
  or `… on (privacy.captureExclusion in config.json) …`; the strip's own line names
  `WDA_EXCLUDEFROMCAPTURE` on Windows. W5 keeps the doctor line and the affinity read-back.
  The W1 run was in a VM on ARM64, not on the planned x64 PC with two monitors, so the
  by-hand list still stands for that PC.

2. **Deviations in building milestone W2** (developer, 2026-10-21). Numbered W2-n.

- **W2-1, the console CLI is a feature-gated `[[bin]]`.** WD7 says a `[[bin]]` in the shell
  crate. It is `bellwether` (`src/bin/bellwether.rs`) with `required-features =
  ["console-cli"]`, and the package has `default-run = "bellwether-shell"`, so macOS builds
  and `cargo test` are unchanged; only `scripts/build-windows-test.mjs` turns the feature
  on (W8's build script will too). With no subcommand it prints the usage; it never starts
  the strip.
- **W2-2, the test folder keeps the repository layout; the test build refuses in code.**
  WD12 5 and W1-3 planned a compile-time test port and home. The folder's `.cmd` files set
  `BELLWETHER_AGENT_DIR`, `BELLWETHER_HOME` and (for the CLI) `BELLWETHER_CONFIG`, as in
  W1, and an exe started any other way refuses every command without `BELLWETHER_HOME`
  (M11-a). What a compiled-in home was for is enforced by the `test-build` feature instead
  (`cli::test_build_refusal`, tested): `hooks install|uninstall`, `token rotate` and
  `migrate` need `--settings <file>`; `autostart install|uninstall` is refused; port 47417
  is refused from `--port` or the config, in the CLI, in `serve` and in the strip.
- **W2-3, `cli.cmd`, not `bellwether.cmd`.** In the folder `.\bellwether` runs
  `Bellwether.exe`: cmd tries `.EXE` before `.CMD`, and NTFS ignores case (found in the
  VM). The wrapper is `cli.cmd`: `.\cli doctor`.
- **W2-4, a Windows `cwd` is cut at `\`** (found in the VM). The label was `basename`,
  which cuts at `/` only, so a Windows session was labelled `C:\Users\…\my-project`.
  `normalise::cwd_basename` also cuts a Windows path (`C:\…`, `C:/…`, `\\server\…`) at `\`;
  a macOS path as before (divergence 28).
- **W2-5, the one flake of the first VM run.** `crates/brain/tests/http.rs` sent the head
  and the body of a request in two writes. A guard answers 403 before reading the body, and
  Windows closes a socket with unread data with a reset, which could lose the 403
  (`request(…).unwrap()` on None, once in a full run; five runs of the target alone
  passed). The test client sends one buffer, as HTTP clients do. The brain is unchanged:
  a real hook with a wrong token fails either way.
- **W2-6, a BOM was refused; now it is kept.** WD8 says the installer keeps a BOM, but
  since M1 a settings file starting with U+FEFF was refused with exit 3 (Node's
  `JSON.parse`), pinned by `test/tester-m1.test.mjs`. Reports on Claude Code's tracker say
  its CLI reads a settings file with a BOM (its desktop app and plugin manifests don't),
  and Notepad and PowerShell 5.1 write them. So, on every OS, the installer and doctor read
  past it and the installer writes it back (`lossless::split_bom`); the M1 test now pins
  that (divergence 27). Not verified here with a real Claude Code: step 10 of the by-hand
  list uses such a file only as a scratch copy.
- **W2-7, where the DACL goes.** WD8 names the token and `backups\`. The rule is "0600 is
  a protected DACL for the user only": every write through `persist::write_atomic_sync`
  with mode 0600 (the token, state.json, `migrate`'s config.json), set on the temporary
  file before any text goes in; a directory `create_private_dir` creates (one that was
  there keeps its ACL, as DirBuilder's mode on Unix); `backups\` when it is created, and
  each backup; `hooks.json` from `hooks print --out`. The user's settings.json keeps its
  inherited ACL. Setting it is best effort (a FAT32 stick has no ACLs), and doctor says
  what the token's DACL is: SYSTEM and Administrators count as "only you" (they can read
  any file, as root can). Messages say `only you can read it` on Windows, not `mode 0600`.
- **W2-8, a closed loopback port reads as "not running" on Windows.** Windows retries a
  connect to a closed localhost port for about 2 s before refusing it, so doctor's 1 s
  probe timed out and said something "accepted the connection but didn't answer /health"
  (seen in the VM with no brain). A loopback listener accepts at once, so on Windows a
  timed-out connect to 127.0.0.1 counts as refused (`client::connect_failure`, tested).
  Checked in the VM with the rebuilt folder (W2 status).
- **W2-9, more managed settings, on Windows only.** WD8 names the Program Files file and
  both registry values. Doctor also reads `managed-settings.d\*.json` beside it, and treats
  each registry value as a managed level with the same checks
  (`settings_checks::check_other_levels_with`). macOS's managed checks are unchanged.
- **W2-10, the bundled pets in the test folder.** Doctor looked for them in the checkout
  the exe was built from (CI's path), and said `reinstall Bellwether`. `pets::bundled_file`
  honours `BELLWETHER_AGENT_DIR`, as the strip does.
- **W2-11, the child brain has no console window.** A development build is a console
  program; the strip starts its brain with `CREATE_NO_WINDOW`.
- **W2-12, `user_home` falls back to `HOME` on Windows.** Without `USERPROFILE` (a test that
  clears the environment), `HOME` is read if that is all there is. The Node test helper
  sets `USERPROFILE` to the test's `HOME`, and the two brain tests that set `HOME` set
  `USERPROFILE` too.
- **W2-13, the rename retry.** Every 50 ms for up to 1 s, on Windows errors 5 and 32 only
  (`persist::replace_file`), for `write_atomic_sync` (the token, state.json) and the
  settings file's install. `config.json`'s edit (M13) and `installs.json` don't rename.

Fix round after BR-3K's W2 verdict (2026-10-22, "ship with fixes"; the maintainer approved
D1 to D4, the doctor note and the optional `Below` removal):

- **W2-14, the strip's stop uses the current token** (D1). `stop_child` sent the token the
  brain was started with; after `bellwether token rotate` the brain takes that one for a
  minute only, so a later quit was refused (403), the brain was ended by `TerminateProcess`
  and state.json lost what changed since its last write. Now the stop is tried with the
  token as it is now (`settings::token_now`, hook-token read again), then the one it started
  with; a 403 moves to the next, any other error ends it (`brain::stop_tokens`,
  `send_stop`). Test: `the_stop_uses_the_current_token_and_survives_a_stale_one` (a brain
  that takes only the new token: the stale token alone is refused and the brain serves on;
  stale then current stops it and state.json has the change). BR-3K's brain-half repro in
  `test/tester-w2.test.mjs` is unchanged (the brain's grace is the same).
- **W2-15, a settings file in use says so** (D2). When Windows still refuses the rename
  after W2-13's second of retries, `hooks install|uninstall` said `cannot write
  \\?\C:\…: EACCES` and exited 1. Now: `<the path as given> is in use by another program (an
  editor, or a virus scan), so it wasn't changed. Close it there and run the command again`,
  exit 3, as the other "nothing was written" refusals (`hooks::write_failure`, tested).
  Every write-failure message names the path as given, never the `\\?\` form. (A backup of
  the untouched file is still made first, as designed.)
- **W2-16, a token file with a BOM is read past it** (D3). Notepad and PowerShell 5.1 save
  `hook-token` with a BOM, which made the token fail the format check (`must be at least 16
  characters …`). `token::read_file`, which every reader goes through (the brain, the
  strip, doctor, the CLI), drops a leading BOM before taking the first line and trimming it
  (CRLF and trailing spaces were already trimmed). Chosen over naming the BOM in the
  message: the token is the same token, and the settings installer accepts a BOM too
  (W2-6). `token init|rotate` still write the file without one. Tested.
- **W2-17, the on-top check reads the style only.** W1-15 also walked the z-order above the
  strip for an ordinary window over it (`OnTop::Below`). Windows keeps every topmost window
  above every ordinary one, so for a strip still marked `WS_EX_TOPMOST` that could only
  catch a race (BR-3K's observation). The walk and `Below` are gone: `desktop::on_top(before,
  after)` is `Kept`, `StyleLost` or `Refused`, and the put-back itself is unchanged
  (every 2 s and on every foreground change).
- **W2-18, `desktop` is a public module** (D4), so `tests/tester_*` files can reach
  `on_top` and `OnTopLog` (`bellwether_shell_lib::desktop`). It holds only pure rules.
- **W2-19, doctor says when Claude Code's settings file has a BOM.** Since W2-6 doctor
  passes such a file, but nobody has confirmed every Claude Code version reads one. Doctor
  adds `INFO settings BOM <file> starts with a byte order mark … some Claude Code versions
  may ignore a settings file that has one: if no sheep appear, save it as UTF-8 without
  BOM`. INFO only: the exit code and the `next` line are unchanged. This changes one
  assertion in BR-3K's `test/tester-w2.test.mjs` (it pinned that doctor says nothing about
  a BOM); it now expects the INFO line.
- Tested after the fix round: Mac `npm run test:cargo` 325 passed, 0 failed (25 targets,
  BR-3K's `tester_w2.rs` included); `npm test` 1049 tests, 1048 pass, 0 fail, 0 cancelled,
  1 skipped; BR-3K's gated rotation test (`BELLWETHER_SLOW=1 … --test-name-pattern="after a
  rotation" test/tester-w2.test.mjs`) 1 of 1; the xwin check (`--locked`) clean. VM:
  `cargo test --workspace` 279 passed, 0 failed (25 targets); with `conformance` 281
  passed, 0 failed; `test/hook-route-blackbox.test.mjs` and `test/tester-w2.test.mjs`
  against the conformance exe, scratch profile: 30 tests, 29 pass, 1 skipped, 0 fail (the
  gated ones stay gated). By hand in the VM, scratch profile: `scratch.json` held open for
  5 s by another process: `… is in use by another program …`, exit 3, the file unchanged;
  closed, the install went through (exit 0). A `hook-token` written by `Set-Content
  -Encoding UTF8` (`EF BB BF` first): doctor took the token, and warned only that the file
  (inherited ACL) is readable by Users and Authenticated Users.
- Not done: splitting `test/brain-server.test.mjs`'s fixture loop. With
  `--test-concurrency=1` Node's `--test-timeout` covers the whole file, so one test per
  fixture would not shorten it (it would add a brain per fixture); BR-3K found CI far from
  the limit.
