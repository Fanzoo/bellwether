# Developing Bellwether

This is for contributors: building Bellwether from source, running it from a checkout,
and the tests. To use Bellwether, install the app instead (see the README).

Bellwether is one Rust executable: the brain (`crates/core`, `crates/brain`) and the app
(`shell/src-tauri`, Tauri 2), which is the CLI, the brain's server and the strip. The
*strip* is a transparent, click-through window above the Dock with one coloured sheep per
session, a bubble on the one that needs you, lambs for its subagents, and a menu-bar icon.
Its web side is `shell/src/`. Node runs the test suite and the development scripts in
`scripts/`; the app doesn't need it.

The Node brain (`brain/` and its shell shim: the repository install) was the first
version and the Rust port's conformance oracle. It was deleted in v1.0.0; its last version
is tag `v0.2.0-rc.3`, and `test/DIVERGENCES.md` records where the Rust brain deliberately
differs from it.

## The two hook guarantees, in code

1. **`/hook` never sends a body.** Claude Code treats a JSON body from an HTTP hook as
   a decision (approve a permission, block a tool call). `crates/brain/src/hook.rs` has
   one way to answer, a `HookAnswer`: an empty `204`, or an empty `403` when a request
   fails a guard. Its `into_response` makes a response whose body type is `Empty`, and a
   source check fails if the `/hook` route builds any other response.
2. **Every bellwether hook entry has `timeout: 1`.** A hung brain therefore stalls a tool
   call by one second at most (measured: exactly 1.000 s), not the 600 s default.
   `doctor` fails any entry without a timeout or above 2 s.

The listener binds `127.0.0.1` only; that is a constant, not configuration. Requests
with an `Origin` header, a foreign `Host`, a non-JSON `Content-Type` or the wrong
`X-Bellwether-Token` get `403`.

---

## First run (from a checkout)

Prerequisites: Rust (`cargo --version`, 1.99 or newer; <https://rustup.rs>), Xcode's
command-line tools (`xcode-select --install`), and Node 22.9 or newer for the tests and
scripts. Run from the repository root:

```
npm run build:dev                                  # cargo build --manifest-path shell/src-tauri/Cargo.toml --features bellwether-shell/conformance
export BELLWETHER_HOME="$PWD/data"                 # the checkout's own files, never the app's
export BELLWETHER_CONFIG="$PWD/config.json"
cp config.example.json config.json                 # port 47420: never the installed app's 47417
alias bw="$PWD/shell/src-tauri/target-dev/debug/bellwether-shell"
bw token init                                      # data/hook-token
bw doctor
bw serve                                           # terminal 1
bw hooks print --out data/hooks-only.json && claude --settings data/hooks-only.json   # terminal 2
cargo run --manifest-path shell/src-tauri/Cargo.toml   # the strip (repository layout: same home, config, token)
node scripts/fetch-pets.mjs                        # optional: the eSheep, into data/pets
```

In order, and what you should see:

1. **Build.** `npm run build:dev` builds the development binary into
   `shell/src-tauri/target-dev/debug/bellwether-shell` (a few minutes the first time), with
   the `conformance` feature the tests need. A plain `cargo build` works for running it too.
2. **Its home.** A development binary is not inside `Bellwether.app`, so it refuses every
   command but `help` and `version` until `BELLWETHER_HOME` is set (exit 3, naming the
   fix): otherwise it would act on the installed app's folder in Application Support.
   `BELLWETHER_CONFIG` points it at the checkout's config.
3. **The config.** `config.example.json` uses port 47420, so a checkout never answers on
   (or installs hooks for) the installed app's 47417. Every key in `config.json` is
   optional.
4. **The token.** `bw token init` writes `data/hook-token` (mode 0600), only if there is
   none, and never prints it. An exported `BELLWETHER_HOOK_TOKEN` wins over the file.
5. **Check.** `bw doctor`: config and token pass; the brain line fails (nothing is
   running yet) and so does `hooks installed`, and that's expected. Its `next` line names
   the step you're on.
6. **The brain.** `bw serve` in one terminal: one line per event, never a command or a
   message.
7. **Hooks, without touching your settings.** `bw hooks print --out data/hooks-only.json`
   writes the hook block as its own settings file (mode 0600: it holds the token), and
   `claude --settings data/hooks-only.json` runs a session with it. `bw status --watch`
   (a third terminal) shows the flock. **Don't** leave a session running with these hooks
   after you stop `serve`: with the brain down, every prompt and tool call prints a
   visible `… hook error …` line.
8. **The strip.** `cargo run --manifest-path shell/src-tauri/Cargo.toml` (or the binary
   with no arguments) opens the strip with the same home, config and token. A sheep walks
   in when the session in step 7 starts. Quit it from its menu-bar sheep or with Ctrl-C.
9. **The eSheep (optional).** Bellwether's own sheep is in `pets/bellwether/` and is the
   default. `node scripts/fetch-pets.mjs` downloads the pinned eSheep into
   `$BELLWETHER_HOME/pets/` and checks its sha256; then set `"pets": {"default":
   "esheep64"}` in config.json.

Never use port 47417 or your real `~/.claude/settings.json` for development: use
`hooks print --out` and `claude --settings`. To try the installer, give it a copy:
`bw hooks install --settings <copy>` shows the diff, `bw hooks install --write --settings
<copy>` writes it (a backup in `data/backups/` first), and `bw hooks uninstall --write
--settings <copy>` takes the entries out again.

---

## Working from a checkout

### The development binary

`shell/src-tauri/target-dev/debug/bellwether-shell` (or `release/`) is the app's own
executable, outside a bundle:
- it needs `BELLWETHER_HOME` for every CLI command but `help` and `version` (above). Inside
  a bundle nothing changes: the Homebrew symlink resolves into the bundle, so it is the app;
- it refuses what only the app can do: `autostart install|uninstall` (no login items to
  change, exit 2), `shell start|stop|restart` (those control Bellwether.app's strip), and
  `migrate --to-app` or `--undo` with your real HOME;
- it never updates itself (`shell.log` says `updates: off (…)` and why);
- its strip reads the *repository layout* (`shell/src-tauri/src/settings.rs`): its home
  is `BELLWETHER_HOME`, else `BELLWETHER_DATA_DIR`, else the checkout's `data/`; its token
  an exported `BELLWETHER_HOOK_TOKEN`, else that home's `hook-token`; its config
  `BELLWETHER_CONFIG`, else the checkout's `config.json`, else `config.example.json`. It
  logs to `<home>/shell.log`. A checkout's `.env` and the old `PASTURE_*` names are not
  read;
- started before there is a token, the strip waits: it looks for one every 3 s;
- if no brain answers on the port, the strip starts one (its own binary as `serve`, with
  the same home and config), and stops it again when the strip quits, however it quits. A
  brain you started yourself is never stopped by the strip;
- it loads only pinned pets whose file matches its sha256 in `pets.lock.json`:
  Bellwether's own from `pets/` (pinned under `bundled`), the eSheep from
  `<home>/pets/` (the pins `node scripts/fetch-pets.mjs` uses). A changed or unknown pet
  file is refused with a line in `shell.log`: rebuild the sheep (`node
  scripts/build-pet.mjs build bellwether`) or fetch the eSheep again.

Debug flags on the strip's binary (they start the strip, not the CLI):

| Flag | Effect |
|---|---|
| `--demo` | one sheep per behaviour from a made-up flock; the brain isn't contacted |
| `--selftest` | parses every configured pet with the webview's real XML parser, prints what it found, exits 0 or 1 |
| `--snapshot <file.png> [--snapshot-after S] [--snapshot-every S]` | writes what the strip draws (plus a `-checker` copy that shows the transparency); `--snapshot-every` keeps writing `-t01`, `-t02`, … |
| `--locate '<terminal json>'` | prints what one terminal lookup finds, and quits |
| `--quit-after S`, `--no-spawn-brain` | for scripted runs |
| `--onboarding` | opens the setup window (the app's; for testing) |
| `--ax-prompt` | asks macOS for Accessibility for this binary (sheep on the Dock) |

Set `BELLWETHER_SHELL_TRACE=1` to log hitboxes, every click-through switch, every
snapshot the webview fetches, and a once-a-second heartbeat with the page's
visibility. `bellwether replay <fixture> --tag <name>` plays a fixture as its own
session and sheep, so several fixtures can drive the strip at once. Each sent event
is printed with its time, for timing checks against `shell.log` (`alert shown`,
`alert confirmed` and `alert cleared` lines, no bubble text). New and confirmed alerts
skip the brain's 250 ms snapshot throttle; snapshots are still capped at 4 a second.

**Test bundles.** `node scripts/build-app.mjs --adhoc --test-port 47418 --test-home <dir>`
builds an ad-hoc `Bellwether.app` whose brain agent and strip use that port and home,
whatever starts them, so it never meets the installed app (see "The signed app").

**Test stand-ins.** Under a temp HOME only (never your real one), the app takes stand-ins
for what it would otherwise ask macOS: `BELLWETHER_LAUNCHCTL` (launchctl),
`BELLWETHER_LOGIN_ITEMS` (SMAppService), `BELLWETHER_STRIP` (starting and quitting the
strip) and `BELLWETHER_UPDATE_*` (the updater). The `conformance` feature also lets
`serve` read test timings from `BELLWETHER_TEST_*` variables. No release build has it.

**Guard rails, kept:**
- every build goes to `shell/src-tauri/target-dev/`: a `.cargo/config.toml` at the root,
  in `shell/` and in `shell/src-tauri` says so, and nothing in the repository builds into
  `shell/src-tauri/target/release` any more (an old repository install's strip lived
  there);
- never set `CARGO_TARGET_DIR`;
- never use port 47417 or your real `~/.claude/settings.json` for development (use
  `hooks print --out` and `claude --settings`).

### What a sheep does

- It **walks** (or **runs**, for Bash) while its session works, and **sits** when
  the turn ends.
- It **grazes** after 20 s idle and **sleeps** after 10 min.
- It **lies down** while the session waits for you to type, is **dizzy** after an API
  error, **chews** while compacting, **trips** when a tool fails, and **leaves** when
  the session ends.

When a session needs you, its sheep stops, turns to face you and bounces, with a
bubble saying what it wants:
- **What the bubble shows:** for example `Bash: git push origin main?  · meadow` (the
  question, then the project in grey). With `bubble.detail: "tool"` it's just
  `Bash?`. Long questions end in "…".
- **Colours:** amber for permission prompts, blue for MCP elicitations, purple for
  "a background session needs input".
- **After about 6 s:** Claude Code's notification confirms the prompt is still open.
  The sheep does one roll, then bounces higher, and the bubble turns bold with a
  thicker border. There is no sound.
- **A subagent waiting on you:** its lamb stops, faces you and bounces under its own
  bubble, `Explore: Bash: npm test?  · meadow`. The parent wears a small dashed
  "lamb" tag, so you can see from the parent that one of its lambs is waiting. Only
  if that lamb isn't on the strip does the parent carry the question itself, in a
  dashed bubble.
- **Many alerts at once:** bubbles share two rows. Only bubbles on the same row
  crowd each other: the project label gives way first, then the longest questions
  are shortened. If one still can't fit, its sheep gets a "!" badge.
- **Clicks:** bubbles are not clickable; clicks pass through them like the rest of
  the strip.

**Lambs** are a session's subagents: smaller sheep (`pets.lambScale`, 0.6) in their
parent's colour. They trot out of the parent on `SubagentStart` into a row beside it,
first to the right and then to the left, and follow it as it wanders. Each family
gets its own stretch of the strip (wider sheep slots while lambs are out, and wide
gaps when the strip is mostly empty), so families don't mingle. A lamb's label is its
agent type, cleaned like bubble text and cut to 24 characters. On
`SubagentStop` they trot back into it and are gone. Beyond `behaviour.maxLambsShown`
(5), a grey "+N" stands where the next lamb would. Hover over a lamb to see its agent
type.

Hover over a sheep to see its project's name; click it and it hops. Each session
keeps its colour and slot, also across `/clear` and restarts.

**Colours:** each session's colour goes on the pet's magenta parts only (hue 285–315°,
`shell/src/player/horns.mjs`; `pets.tint` `"horns"`, the default, keeps that name): the
**flight helmet** on Bellwether's sheep (its mask tones, `pets/SPEC.md`), the **horns** on
the eSheep. The rest of the pet stays as drawn, and a pet with no magenta takes no colour.
There are eight colours (`pets.palette`): orange, blue, black, red, cyan, brown, green and
pink, in the order sessions get them. To tint the whole sheep instead, as before, set
`"pets": {"tint": "whole"}` in `config.json`, then restart the strip. A
`config.json` still carrying the old default palette (white first, then pale blue) gets
the new colours; doctor says so, and your file is left alone.

**Grazing:** a sheep (or lamb) that is eating gets a small tuft of grass under its
snout. It fades in, and fades out a moment after the sheep moves on.

**The Dock:**
- With the Dock shown at the bottom, the strip sits just above it.
- With auto-hide on, the strip sits on the bottom of the screen, and the Dock slides up
  over the sheep when you hover.
- A Dock on the left or right leaves the strip on the bottom edge, clear of the Dock.
- Changes are followed within a few seconds, without a restart.
- **On the Dock itself:** with Accessibility granted, sheep stand on the Dock's top
  edge, within its width, and turn at its ends. Families that don't fit stand beside
  it. The app asks for it in its setup window. A development strip asks when you run
  its binary with `--ax-prompt` (`shell/src-tauri/target-dev/debug/bellwether-shell
  --ax-prompt`); turn on bellwether-shell in System Settings > Privacy & Security >
  Accessibility, then restart the strip.
- A development build's grant belongs to that build, which is ad-hoc signed, so **after
  every rebuild it must be given again**. Until then, the full-width strip is used.
- **Granted before, and now `doctor` (or `<home>/shell.log`) says "not granted"?** The
  strip was rebuilt, and System Settings > Privacy & Security > Accessibility still
  shows the old bellwether-shell entry, switched on. That entry belongs to the previous
  binary, so switching it off and on does nothing. Remove the old `bellwether-shell`
  entry (select it, then −), run the new binary with `--ax-prompt`, turn the new entry
  on, and restart the strip.

**Two sessions in one project** get told apart in their labels and bubbles:
`my-project · p2` and `my-project · p4` (the herdr pane). Otherwise it's the
iTerm tab (`tab 2`), the tmux pane, or a number (`#2`). A session alone in its project
keeps the plain name.


### Staying out of the way

- **Menu-bar icon** (a small sheep): *Quiet (alerts only)*, *Hide everything (for screen
  sharing)*, *Show labels*, *Quit Bellwether* (Bellwether.app adds *Set up Bellwether…*).
  - **Quiet (alerts only)** is the brain's do-not-disturb, the same switch as `bellwether
    mute` / `bellwether unmute`, kept by the brain in `state.json` in its home. The strip stops
    animating. Only a sheep (or lamb) that needs you still shows a small "!", and clicks
    pass through the whole strip.
  - **Hide everything (for screen sharing)** hides the whole strip, "!" included. It and
    **Show labels** are kept in `shell-state.json` in the home, so they survive a restart. A
    hidden strip comes back hidden.
  - **The way back without the tray:** `bellwether show` (and `bellwether hide`) do the
    same as the tray's Hide everything. A running shell follows within a second, and a
    stopped one starts that way. The shell log's "strip hidden" line says so too. If the
    tray can't be created while the strip is saved hidden, the shell shows the strip
    anyway and logs an ERROR line.
  - Autostart isn't in the menu; use `bellwether autostart`.
- **Screen sharing:**
  - The strip asks macOS to leave it out of screen captures (`privacy.captureExclusion`,
    `sharingType none`). Newer macOS capture APIs may ignore that, so it isn't relied on.
    The shell reads its config only when it starts: after changing `privacy.*` (or
    anything else in `config.json`), restart the strip (`bellwether shell restart` for the
    app's).
  - It also hides itself while a process named in `privacy.sharingProcesses` runs:
    Zoom's `CptHost`, which runs while Zoom shares. It comes back 2 s after that process
    exits.
  - Process names are compared whole, ignoring case, and macOS cuts them at 31 bytes.
    `bellwether doctor` and the shell log warn about any entry that can never match:
    empty, padded with spaces, a path, longer than 31 bytes, or a `.exe` name on a
    Mac.
  - Teams and Google Meet in a browser have no such process. Before sharing in those, use
    *Hide everything (for screen sharing)*, or set `bubble.detail` to `"tool"` so a bubble
    never shows a command.
- **Full-screen apps:** the strip stays off a full-screen app's Space.
- **Near the cursor:** a sheep (and its bubble) fades to `fade.opacity` while the
  cursor is within `fade.radiusPx` of it, so you can see and click what's underneath.
- `BELLWETHER_CONFIG=<file>` points the shell at another config file, read-only. It's a
  test aid, for example to add a dummy name to `privacy.sharingProcesses`.

### Which monitor, and clicking a sheep

- **Each sheep lives on the monitor where its terminal window is**, in a strip along
  that monitor's Dock edge.
  - The shell finds the window with no permissions. Inside herdr it goes through the
    herdr client process; otherwise through the session's `claude` process. Either
    way it then reads the on-screen window list.
  - It looks again when a sheep first appears, on each new alert, every
    `strip.relocateSec` (60 s), and when the displays change.
  - A sheep it can't place lives on the main monitor.
- **Click a sheep** to bring its terminal to the front. The click fires when the button
  comes up, not when it goes down (a press can't know yet whether it will be a drag).
  - Inside herdr: herdr's workspace, tab and pane are selected, then the window
    hosting herdr comes forward.
  - In a plain iTerm2 tab, that tab is selected; in Terminal.app, the window holding
    that session.
  - Otherwise the terminal app is brought forward.
  - A sheep with no terminal headers just hops.
- **Automation permission.** Selecting the exact iTerm2 or Terminal window uses
  AppleScript, so macOS asks once to let the shell control iTerm2 (or Terminal).
  - If you refuse, everything still works by app: sheep go by the app's front window,
    and a click brings the app forward. `bellwether doctor` then names the missing
    permission.
  - `"locator": {"appleScript": false}` turns AppleScript off entirely.
- `locator.strategies` sets the order of the lookups, and `locator.herdrPath` sets
  where herdr is (default `herdr`, also found in `~/.local/bin`).
- `bellwether-shell --locate '<terminal json>'` (a debug flag) prints what one lookup
  finds, and quits.

### Moving a sheep

- **Drag it:** press on a sheep and move 4 px or more. It plays `carried` and dangles
  under the cursor, above every bubble and at full opacity. Let go and it falls to the
  ground of the strip under the cursor, plays `land`, then carries on with whatever its
  session is doing, around that spot. A press and release that moves less than 4 px is a
  click, as before.
- **Where it can go:** along the strip, left or right, including onto another monitor's
  strip (one opens on every monitor while you carry a sheep). It always lands on the
  ground; never mid-screen.
- **Onto another sheep:** it lands, then walks to the nearest spot where its family fits
  clear. Sheep in their slots never move for a dropped one. With no room anywhere on that
  strip it stays where it was dropped, in front, and the log says `no room`.
- **How long it lasts:** until the app restarts (`bellwether shell restart` included), its
  session ends (`/exit`, a crash), the monitor goes, or you drag it again. `/clear` and
  `/resume` keep it. Nothing is saved: no slot changes, nothing in `shell-state.json` or
  `state.json`. While it lasts, the sheep doesn't move back to its terminal's monitor.
- **Lambs** follow their parent: along the strip while it is carried, and to its new
  monitor. A lamb can't be dragged; pressing one and moving does nothing.
- **Quiet** and **Hide everything** leave nothing to pick up, and turning either on
  mid-drag puts the sheep back where it was picked up. There is no Escape: to undo a move,
  drag it back, or restart the strip.
- **The log** (`shell.log`) has `sheep <id> picked up`, `sheep <id> dropped on <monitor>
  at x <n>`, `drag cancelled: <reason>` and `drop for <id> ended: <reason>`, never a
  bubble's text.
- `--demo --demo-drag` (debug, with `--snapshot <png> --snapshot-every 1`) runs a scripted
  drag on the demo flock: it presses sheep 2, carries it onto sheep 4, drops it, then
  clicks sheep 1.

### Things to check by hand once

Against a development strip (above) or a test bundle.
These need your hands, your eyes or a screen permission, so they aren't automated:

1. Open two Claude sessions in two iTerm2 tabs (with hooks). Two differently coloured
   sheep appear directly above the Dock, each wandering near its own slot.
2. Put a Finder icon or terminal text behind the strip, between two sheep, and click
   it. The click reaches it.
3. Click a sheep. It hops, and typing still goes to your terminal.
4. Quit the shell and start it again. The sheep come back in the same colours and
   slots.
5. Make any app full screen (green button). The strip doesn't appear on that Space.
6. In a session with hooks, ask for an Explore subagent that runs `ls` in a scratch
   directory. A lamb labelled `Explore` appears beside that session's sheep and goes
   back into it when the subagent finishes. If the subagent needs permission, the
   lamb alerts and the parent wears the "lamb" tag.
7. **Capture checks.** Use a session with one sheep waiting on a
   permission prompt, so there is a bubble to see. For each check, note whether the
   strip is visible.
   1. **macOS screenshot tool:** Cmd-Shift-3 (whole screen), then Cmd-Shift-5 (a
      region over the strip). Open the images.
   2. **QuickTime:** File > New Screen Recording, record 5 s of the whole screen, and
      play it back.
   3. **Zoom:** in a meeting with yourself (a second device, or a test meeting), share
      the whole screen and look on the other device. Then run
      `ps -axco comm | grep -i -e cpthost -e zoom` while sharing, and note which names
      appear, so `privacy.sharingProcesses` can be checked.
   4. **Teams:** share your screen in a test call and look from the other side. Run the
      same `ps` line (with `teams`) to see whether any process appears only while
      sharing.
   5. **Google Meet in Chrome, then in Safari:** present your entire screen and look
      from the other side.
8. **Tray.** Click the menu-bar sheep.
   1. *Quiet (alerts only)*: the sheep vanish, and a waiting one shows only a "!". Run
      `bellwether status`: it shows MUTED. Quit and restart the shell: still muted.
      *Quiet* again: back.
   2. *Hide everything (for screen sharing)*: the strip disappears, "!" included. Quit
      and restart the shell: it stays hidden. *Hide everything* again: back.
   3. *Show labels*: every sheep shows its project.
   4. *Quit Bellwether*: the shell quits. A brain started by the shell stops too.
9. **Full screen.** Make Safari or Terminal full screen (green button). The strip isn't
   on that Space; swipe back and it's there.
10. **Fade and clicks.** Move the cursor next to a sheep: it fades. Click the sheep: it
    still hops. Click just beside it: the click reaches what's underneath.
11. **Two monitors.** Plug in a second monitor and start the shell.
    1. Put a session's terminal on the second monitor: its sheep appears in a strip
       on that monitor. A session on the main monitor stays there.
    2. Drag that terminal to the other monitor. The sheep moves within a minute, or
       at once on its next alert.
    3. When no sheep live on the second monitor, its strip goes.
12. **Click inside herdr.** Make a session in a herdr pane ask for permission, switch
    to another herdr tab, and click its sheep. The iTerm2 window hosting herdr comes
    to the front, with that session's herdr tab (and pane) selected.
13. **Click in a plain iTerm2 tab, then in Terminal.app.**
    1. The first click asks to let the shell control iTerm2 (or Terminal). Allow it.
    2. In iTerm2, the click selects the session's tab.
    3. In Terminal.app, it brings Terminal and the right window forward.
14. **Refusing Automation.**
    1. In System Settings > Privacy & Security > Automation, turn off the shell's
       access to iTerm2, and restart the shell.
    2. Sheep still sit on their monitors. A click brings iTerm2 forward (not the
       exact tab).
    3. `bellwether doctor` shows an `automation` warning naming iTerm2.
15. **A missing herdr CLI.** Set `"locator": {"herdrPath": "/nope/herdr"}` in
    `config.json` and restart the shell. herdr sessions are still placed (through
    iTerm2 or the process tree), and `bellwether doctor` shows `herdr CLI`. Remove the
    setting afterwards.
16. **Moving a sheep** (milestone 12). Use a test build on a free port with a scratch
    home; make sheep with `bellwether replay <fixture> --tag <name>` against that port, and
    one real session with `claude --settings <scratch hooks file>` for the clicks.
    1. Click versus drag, with a mouse and with a trackpad (tap, click-and-drag,
       three-finger drag): a click hops and focuses on release; a drag never focuses.
    2. Drop onto empty ground, onto another sheep (it walks clear; the other doesn't
       move), and onto a full strip (`no room` in the log).
    3. Drop a sheep while it alerts: a "!" badge while carried, its bubble after landing.
    4. Turn on Quiet, then Hide everything, mid-drag: the sheep goes back where it was
       picked up.
    5. Two monitors: carry a sheep across (a strip appears there), drop it, carry it back;
       the strip left empty goes.
    6. The fail-safe: reload the strip mid-drag (a pet change in the setup window), let
       go, then click a Finder icon under the strip: the click reaches it.
    7. `/exit` in a dropped sheep's session (a new `claude` in that tab comes back in its
       slot), and `/clear` (the sheep stays where it was dropped).
    8. `bellwether shell restart` puts every sheep back in its slot, on its terminal's
       monitor.

---

## The signed app

`docs/DISTRIBUTION-PLAN.md` made Bellwether one signed, notarized `Bellwether.app`,
released as a DMG and a Homebrew cask. The app runs without Node. A Mac that still runs
the repository version (0.1, from a checkout with Node) moves into it with `bellwether
migrate --to-app`, or the setup window's first step (see "Moving a repository install into
the app" below).

A development build (`target-dev/`) is not inside an app bundle, so it keeps a checkout's
layout ("Working from a checkout" above). Only `Bellwether.app` reads the app's files
below.

**One executable, three modes.** `Bellwether.app/Contents/MacOS/Bellwether`:
- launched from Finder, it is the strip;
- `Bellwether serve` is the Rust brain. It never starts Tauri, windows or WebKit;
- called as `bellwether` (a lower-case symlink on PATH), or with a subcommand, it is
  the CLI: `serve`, `record`, `status [--watch] [--json]`, `mute`/`unmute`, `show`/`hide`,
  `replay`, `token init|rotate`, `hooks print|install|uninstall`, `doctor`, `pets import`,
  `migrate`, `autostart status|install|uninstall`, `version`, `help`. Their flags and
  output are the old Node CLI's, except where `test/DIVERGENCES.md` says otherwise.

Its files live in `~/Library/Application Support/Bellwether/`: `hook-token`, `config.json`
(optional; every key has a default), `state.json`, `shell-state.json`, `recordings/` and
imported pets (`pets/`, pinned in `pets.user.lock.json`). Logs go to
`~/Library/Logs/Bellwether/` (`brain.log`, `shell.log`). `BELLWETHER_HOME` overrides that
folder (logs then go to `<home>/logs/`), and `BELLWETHER_CONFIG` names another config file.

**The setup window** (milestone 7) opens on the app's first launch, and again from the
menu-bar sheep's *Set up Bellwether…* or `open -b <bundle id> --args --onboarding`. It
walks through, in order: start at login (both login items; recommended), the hooks (the
exact diff `bellwether hooks install` would apply to `~/.claude/settings.json`, token
masked, applied only on Install, and refused with a fresh preview if the file changed
since; Uninstall works the same way), sheep on the Dock (Accessibility, optional), click a
sheep to jump to its tab (Automation for iTerm2 and Terminal, optional), the pet (Import…
opens a file panel; the file is checked as `pets import` checks it), the `bellwether`
command in `~/.local/bin` (optional), and a last page with doctor's results in plain words.
A new user's hook token is made on the way (with start at login, or with the first
Install). While a repository install's LaunchAgents are still on the Mac, it names them and
won't turn on start at login or install hooks; instead its first page shows what moving
that install into the app would change, and does it on Move it into the app. It
finishes with Done, which it remembers in `shell-state.json`; closing it otherwise brings
it back at the next launch. A development build never opens it by itself (it does with
`--onboarding`, for testing).

**The strip** (the app opened from Finder, or `bellwether shell start`):
- reads the port and `config.json` from that folder, and the token from `hook-token` (or
  `BELLWETHER_HOOK_TOKEN`). Started before there is a token, it looks again every 3 s; it
  reads the file again when the brain refuses the old one (after `token rotate`);
- runs once per port: a second strip on the same port hands over to the first and quits,
  while strips on different ports (a test build beside the installed app) run side by side;
- uses the brain answering on the port. When none answers, it starts one: its own
  executable as `Bellwether serve`. That brain stops when the strip quits, and within a
  second when the strip is killed outright (`kill -9`). A brain started by the login agent
  or by hand is never stopped by the strip;
- at start, compares its version with the brain's `/health`. If the brain is this app's
  own login agent (launchd's pid for `<bundle id>.brain` is the pid `/health` reports, and
  that pid runs this bundle's executable, or one an update has deleted; a second copy of
  the app with the same id leaves the installed copy's agent alone) running another version (after an update), it restarts the agent (`launchctl kickstart
  -k`) and waits up to 10 s for the new version; twice at most, then a line in the menu
  ("Brain is still …") and in `shell.log`, and doctor's brain line says what to do. Any
  other brain (one started by hand, a repository install's, another copy of the app's) is
  left alone, with a line in `shell.log`. Tests drive this with a stand-in launchctl
  (`BELLWETHER_LAUNCHCTL`, under a temp HOME only); the real one is never called from a
  unit test;
- loads a pet bundled with the app (Bellwether's own sheep, the default), else one you
  imported with `bellwether pets import` (or the setup window's Import…), each only while
  it matches its pin. `pets.default` in `config.json` names it. An imported pet also needs
  an `animationMap` entry; the import prints one, and choosing the pet in the setup window
  writes it for you:

```
bellwether pets import <folder>/esheep64/animations.xml       # pinned in pets.user.lock.json
# config.json: {"pets": {"default": "esheep64", "animationMap": {"esheep64": {…}}}}
#   (the animationMap the import prints, or the one in config.example.json)
bellwether shell start | stop | restart                        # the app's strip
```

The brain reads its token from `hook-token`
(one line, mode 0600), or from `BELLWETHER_HOOK_TOKEN`:

```
bellwether token init                      # make hook-token, only if there is none (never printed)
bellwether token init --from <repo>/.env   # or take the repository install's token, so its hooks keep working
bellwether token init --force              # replace a token file that is there but unusable (init refuses, exit 3)
bellwether token rotate --write            # a new token; a running brain switches and accepts the old one for a minute
```

`token rotate --write` writes the new token file, tells a running brain to switch (it
accepts the old token for one more minute), then rewrites bellwether's hook entries in
`~/.claude/settings.json` (or `--settings <path>`) with the new token, with a backup in
`backups/`. Without `--write` it shows that diff, with the token as `<new token>`.

The installer and doctor work as the Node CLI's did (`hooks install` previews are the same
bytes), with the app's files in place of the checkout's `data/`: the install record is `installs.json`
and backups go to `backups/` in the app's folder. Doctor also checks the token file's mode,
the app's identity and signature, its login items, the strip's Accessibility (from its
log, and whether a strip runs: doctor can't ask macOS itself, which would answer for the
terminal), a brain of another version than the app (WARN), LaunchAgents left by a
repository install, whether the strip could switch WebKit's occlusion detection off, and
fails a bellwether hook entry that sets `"onFailure": "block"`.

```
bellwether pets import ~/Downloads/mypet/animations.xml   # check, copy and pin a desktopPet file (16 MiB, UTF-8, no DOCTYPE, tiles up to 64)
bellwether migrate --from <repository>                    # preview: what a repository install's token, config, colours and hooks would become
bellwether migrate --from <repository> --write            # do it; its LaunchAgents are listed and left alone
bellwether migrate --to-app                               # preview the whole move into the app (D9): token, files, LaunchAgents, hooks
bellwether migrate --to-app --write                       # do it (Bellwether.app's own executable; refuses a development binary)
bellwether migrate --undo [--write]                       # put the LaunchAgents and the settings file back
```

**Moving a repository install into the app** (`migrate --to-app`). Only a Mac that ran
Bellwether 0.1 from a checkout (its two LaunchAgents, `local.bellwether.brain` and
`.shell`) needs this; a new user just installs the app. Do it before pulling v1.0.0: a
checkout at 1.0 has no `brain/`, so its LaunchAgent brain can't start again (the migration
still works afterwards; it reads only the old plists, `.env`, `config.json` and `data/`;
an undo needs `git checkout v0.2.0-rc.3` first, to bring the old brain back). In short:
- It finds the old install from `~/Library/LaunchAgents/local.bellwether.brain.plist` (or
  the older `local.pasture.brain.plist`), or `--from <repository>`; hook entries alone are
  enough. Nothing changes without `--write`, and the repository itself is never changed.
- With `--write`, in order: the token running sessions carry goes into `hook-token` (it
  replaces a differing app token, kept in `legacy/`, so those sessions keep working; the
  preview says so); config.json (without `brain.nodePath` and
  `autostart`), state.json, shell-state.json, installs.json, `backups/` and fetched pets,
  each only if the app has none (`--force` replaces); a port check (anything but the old
  brain on the port stops it, with nothing changed); the old agents booted out and their
  plists moved to `legacy/`, the app's login items registered, and up to 10 s for the new
  brain to answer with the token (if it doesn't, all of that is undone and the old setup
  keeps running); the hook entries rewritten to the current headers (a backup first);
  doctor.
- From the CLI it then starts the app's strip (unless one is running), so the sheep show
  now rather than at the next login. The setup window starts none (it is the strip), nor
  does a run that stops or undoes itself (the old setup keeps its own). If the strip
  doesn't start, the migration still counts as done and says `bellwether shell start`.
- A file that can't be copied stops it before any agent is touched. One run (CLI or setup
  window) writes at a time. A second run says "Nothing to migrate"; one that was
  interrupted (or whose login items or brain are gone since) is reported as unfinished,
  and the next run finishes it.
- `--undo --write` moves the plists back (all or none), unregisters the login items, quits
  the app's strip (the old agents bring their own back; it stays if the login items
  couldn't be unregistered), starts the agents that were running, and restores the
  settings file from the backup if it is still exactly what the migration wrote, then
  lists anything of the app still running. A strip that won't quit within 5 s stops it
  before the old agents start (never two strips): quit Bellwether from its menu-bar sheep
  and run it again. Anything it can't put back is listed, with a non-zero exit, so it can
  run again.
- With your real HOME it runs only from an installed Bellwether.app (a release build); any
  other binary refuses before reading anything. Under any other HOME it calls launchctl,
  the login items and the strip only through test stand-ins (`BELLWETHER_LAUNCHCTL`,
  `BELLWETHER_LOGIN_ITEMS`, `BELLWETHER_STRIP`), never the real ones; without them it
  starts and quits nothing.

**What you need once:**
1. The **Developer ID Application** certificate, with its private key, in your login
   keychain. `security find-identity -v -p codesigning` lists it.
2. The first time `codesign` uses that key, macOS asks whether `codesign` may use it.
   Choose **Always Allow** (it asks for your login keychain password). Until you do,
   every build stops at signing.
3. A notarytool keychain profile named `bellwether`, made from an App Store Connect API
   key (Users and Access > Integrations; the `.p8` downloads only once):
   `xcrun notarytool store-credentials bellwether --key <AuthKey.p8> --key-id <key id>
   --issuer <issuer id>`.
4. The Tauri CLI, into `shell/node_modules` (gitignored):
   `npm ci --prefix shell`.

**Build:**

```
node scripts/build-app.mjs --notarize        # build, sign, notarize, staple, then check with spctl
node scripts/build-app.mjs                   # build and sign only
node scripts/build-app.mjs --adhoc           # no keychain: ad hoc, hardened, under its own id (…bellwether.adhoc)
node scripts/build-app.mjs --universal       # Apple silicon and Intel (rustup target add x86_64-apple-darwin once)
```

- The app goes to `shell/src-tauri/target-dev/release/bundle/macos/Bellwether.app`.
- `--test-port 47418 --test-home <dir>` makes a test build whose bundled brain agent
  runs on that port with that home, so it can't collide with a brain on 47417. Its strip
  takes the same port and home from the bundle, however it is started, and refuses a
  different `BELLWETHER_HOME`.
- The script refuses `--test-port 47417`, and fails if
  `shell/src-tauri/target/release/bellwether-shell` changed during the build.
- `BELLWETHER_SIGNING_IDENTITY` picks one identity (a name or SHA-1) when there are
  several. `BELLWETHER_NOTARY_PROFILE` names another notarytool profile.
- The Team ID is never written to a file, and the script masks it in what it prints.

**Check and try it:**

```
A=shell/src-tauri/target-dev/release/bundle/macos/Bellwether.app
codesign --verify --deep --strict -vvv "$A"
codesign -d --entitlements - "$A"            # only com.apple.security.automation.apple-events
spctl -a -vvv -t exec "$A"                   # after --notarize
"$A/Contents/MacOS/Bellwether" doctor --port 47418
"$A/Contents/MacOS/Bellwether" autostart install --write --brain-only   # the brain at login (SMAppService)
"$A/Contents/MacOS/Bellwether" autostart uninstall --write --brain-only
```

`autostart` registers the brain's login agent, which is the plist inside the bundle at
`Contents/Library/LaunchAgents/`, and the app itself as a login item. It never writes
to `~/Library/LaunchAgents`. `--brain-only` leaves the app login item alone, and
`--via-app` hands the call to the app instead. It refuses an ad-hoc build carrying the
permanent id: macOS would keep that registration's launch constraint and then kill the
Developer ID build's brain ("Launch Constraint Violation"). If that happens anyway,
`autostart uninstall --write`, then `install --write` again.

More about `autostart`:
- `autostart status` prints `enabled`, `off` (`notRegistered`, or `notFound` for an item
  macOS has never had registered), `requiresApproval` or `unavailable` for each item.
- **Waiting for approval.** macOS may register an item but not start it until you allow
  it. `install --write` and `status` then say so, exit 2 (install) and, at a terminal, ask
  whether to open System Settings > General > Login Items & Extensions (Login Items before
  macOS 15); only `y` opens it. Turn Bellwether on under "Allow in the Background", then
  run `autostart status` again. `bellwether autostart settings` opens that pane any time;
  doctor shows the item as a WARN, and the strip's menu-bar sheep has an "Approve the … in
  Login Items…" line that opens it.
- `install` refuses the brain agent until its home has a hook token (`bellwether token
  init`, with `BELLWETHER_HOME` for a test build): without one the agent would exit at once
  and launchd would restart it every 10 s.
- A test build (`--test-port`/`--test-home`) may only use `--brain-only`: a test build's
  strip never starts at login. (Before milestone 5 the reason was that such a strip would
  use the live defaults, port 47417; it now takes the bundle's test port and home.)
- `uninstall --write` also stops this bundle's running strip.
- Outside Bellwether.app (a development binary from `target-dev/`) `install` and
  `uninstall` refuse with exit 2: there are no login items to change.
- In a terminal opened before your last login (a herdr or tmux server that outlived a
  logout), macOS won't answer about login items. `autostart` and `doctor` then say so:
  open a new terminal window.

**Legacy agents.** The repository version's LaunchAgents (`local.bellwether.brain` and
`.shell`, or pasture's `local.pasture.brain` and `.shell`) run a second brain that fights
the app's for port 47417. Doctor's `legacy agents` line and a line at the top of the
strip's menu-bar menu name each one that is still there: its plist in
`~/Library/LaunchAgents` (it loads again at the next login), or loaded in launchd for your
user (with its pid), even with its plist deleted. The app only looks (`launchctl print`);
it never stops, loads, moves or deletes them by itself. Moving them aside is the
migration's job (`bellwether migrate --to-app`, or the setup window's first step), when you
choose to. launchd is asked only when HOME is your real home folder.

**Updates and releases** (the maintainer's steps are in `docs/RELEASING.md`; what users
see is in the README):
- The installed app checks for a new version 10 s after it starts and then once a day,
  and asks before installing ("Install and Relaunch" or "Later"). The menu-bar sheep's
  *Check for Updates…* checks at once and says when there is nothing new. Updates come
  from the newest published GitHub release (`latest.json`), are signed with the project's
  updater key (the app refuses anything else, or a signature made for another version),
  and replace the app where it is. After the relaunch the app restarts its own brain
  agent (the strip does it at start, not the updater before the relaunch), so hooks see
  a gap of well under a second.
- `"updates": {"check": "off"}` in `config.json` turns the automatic checks off; *Check
  for Updates…* still works.
- Only the Developer ID app updates itself. A development build, an ad-hoc or test build
  (`--adhoc`, `--test-port`) and an app without the updater key in
  `tauri.dist.conf.json` never do; `shell.log` says `updates: off (…)` and why.
- Releases are built by `.github/workflows/release.yml` from a pushed `v*` tag: a universal
  app, signed and notarized, a stapled DMG, the updater's archive and signature,
  `latest.json` and `SHA256SUMS`, on a draft release. `node scripts/release.mjs` does the
  same steps by hand (`version`, `check`, `assets`, `cask`).
- Homebrew: `brew install --cask fanzoo/tap/bellwether` (the tap is Fanzoo/homebrew-tap,
  rendered from `packaging/homebrew/bellwether.rb`). The cask puts `bellwether` on your
  PATH. Before `brew uninstall --cask bellwether`, run
  `bellwether hooks uninstall --write` and `bellwether autostart uninstall --write`:
  Homebrew can't edit Claude Code's settings. A plain uninstall keeps your files;
  `brew uninstall --zap` also deletes `~/Library/Application Support/Bellwether`, your
  `config.json` and hook token included (back up `config.json` first if you want it).

### Bellwether's own pets

The app has its own sheep, `bellwether`, in a flight helmet that takes the session
colour (an aviator goose, `brah`, comes later). Its art is committed under CC0-1.0 in
`pets/`, as text sources that `scripts/build-pet.mjs` builds into
`pets/bellwether/animations.xml`; `pets/SPEC.md` describes the source form and the
design. With no `pets.default` in config.json, the strip draws it, from the checkout
(a development build) or from the app's Resources, only while it matches its pin in
`pets.lock.json`. The names `bellwether` and `brah` can't be used by `pets import`.

```
node scripts/build-pet.mjs build bellwether        # after editing pets/bellwether/src or pet.json
node scripts/build-pet.mjs --check                 # npm test and CI run this too
node scripts/build-pet.mjs review bellwether --out /tmp/review   # the review sheets
```

## Licences of dependencies

`THIRD-PARTY.md` lists every Rust crate and npm package with its licence. Regenerate it
with `node scripts/third-party.mjs` after changing a dependency (offline;
`--check` says when it's stale).

## Commands

`bellwether help` lists the CLI (the app's, and the development binary's, which needs
`BELLWETHER_HOME`). The development scripts, all Node with no dependencies:

| Script | What it does |
|---|---|
| `node scripts/fetch-pets.mjs [--data <dir>] [<pet>]` | Downloads the pinned eSheep into `<dir>/pets/` (default `$BELLWETHER_HOME`, else `data/`) and checks its sha256. That art is never committed |
| `node scripts/build-pet.mjs build\|review\|--check` | Bellwether's own pets, from their sources in `pets/` |
| `node scripts/build-app.mjs [--adhoc] [--universal] [--notarize] [--test-port P --test-home D]` | Builds (and signs) `Bellwether.app` |
| `node scripts/release.mjs version\|check\|assets\|cask` | Release steps (docs/RELEASING.md) |
| `node scripts/third-party.mjs [--check]` | Regenerates THIRD-PARTY.md |
| `node scripts/health-gap.mjs --port P --seconds S` | Measures the brain's longest gap in answering `/health` (across an update) |

## Tests

```
npm test               # builds the development binary (npm run build:dev), then the suite against it
npm run test:cargo     # the Rust tests (shell and brain), from the repository root
cargo test --manifest-path shell/src-tauri/Cargo.toml --workspace --features bellwether-shell/conformance
```

`cargo test` on its own has to be run in `shell/src-tauri`, where the Cargo workspace
is; from the root, use `npm run test:cargo`. `npm run test:cargo` builds the same binary
without the `conformance` feature, so the next `npm test` rebuilds it first; a binary
without it makes `npm test` stop at once with the build command.

- **The suite** (`test/*.test.mjs`, one `node --test`, one file at a time, at most 120 s a
  test): the brain's tests, the player and the setup window's web side, the dev scripts,
  the docs, and the isolation guard. Every test that starts the brain or its CLI goes
  through `spawnBrain` in `test/helpers.mjs`, which runs `$BELLWETHER_BIN`, by default
  the development binary (refused when it is older than its sources). The helper makes the
  test's data dir the binary's `BELLWETHER_HOME`, passes the test's config as
  `BELLWETHER_CONFIG`, gives it a temp HOME, refuses anything that would use port 47417 or
  name your real settings files, and runs `autostart` only from a development binary.
- **The brain's parts in process** come from `test/impl.mjs`: the Rust brain, called
  synchronously through the hidden `__session` command (one call per line, over two
  FIFOs); `startBrain` is a Rust `serve` process and `runDoctor` a `doctor --json` run of
  the binary. The tools tests drive a brain with (an HTTP client, replay) are in
  `test/support/`, copied from the Node brain: drivers, never the thing under test.
- **The isolation preload** (`test/setup.mjs`) gives every test process a temp home and
  config on a port that isn't 47417, and fails the run on any access to the checkout's
  `.env`, a write under its `data/` (reads only in `data/pets`), a connection to 47417, or
  any read or write of the real `~/.claude/settings.json`.
- **Golden vectors.** `test/fixtures/` holds `redact-`, `normalise-`, `config-`, `env-`
  and `diff-vectors.json` and `installer-vectors/`: cases with the answers the Node brain
  gave, checked on every run by cargo (`crates/core` and `crates/brain` `tests/vectors.rs`,
  `conformance.rs`) and by the suite through `test/impl.mjs`. A new or changed case is
  written by hand, with the answer the Rust brain gives, and reviewed like code.
- **The frozen sample** (`test/fixtures/frozen/`): 2 000 generated cases per area (env,
  redact, normalise, json, diff) with the Node brain's answers, frozen before it was
  deleted; `crates/core/tests/frozen.rs` requires the same answers. Don't edit it; a case
  the brain should now answer differently goes into the vectors instead.
- **The golden fixtures** (`test/fixtures/s*.jsonl`) are rebuilt with `node
  test/fixtures/build-fixtures.mjs && node test/regen-expected.mjs`; read the diff.
- `test/DIVERGENCES.md` is history: where the Rust brain deliberately differs from the
  Node brain, each entry with the test that pins it. No new entries.
- The `conformance` feature adds the hidden `__` commands (`__session`, `__json`, and
  `__env`, `__redact`, `__normalise`, `__fixture`, `__diff`, `__list`). No release build
  has it: `Bellwether __fixture` answers "unknown command" with exit 1.

**CI** (`.github/workflows/ci.yml`, GitHub Actions, no secrets): `suite` (`cargo fetch`,
`npm run build:dev`, `node scripts/fetch-pets.mjs`, `npm test`, `build-pet --check`),
`cargo` (the cargo tests with and without the `conformance` feature), `bundle` (an
unsigned universal `tauri build` with its DMG and updater files, signed with a key made and
thrown away in the job, and its refusal of `__fixture`), and `cross-check` (`cargo check`
of `crates/core` and `crates/brain` for Linux and Windows). The redaction time budgets are
written for a developer's Mac and scale with the machine's speed, measured in the same
process (`machineSlowdown` in `test/helpers.mjs`).

Runs serially (`--test-concurrency=1`): several suites use live sockets and timing.
Offline and self-contained. The installer tests only ever use temp copies of an
invented settings file shaped like yours; nothing reads or writes
`~/.claude/settings.json`.
