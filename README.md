# Bellwether

A desktop-pet strip that shows each of your Claude Code sessions as a sheep along the
bottom of the screen. The sheep for a session that is waiting on you (a permission
prompt, a question) turns to face you with a bubble saying what it wants; click it to
jump to that terminal.

- **The name:** Bellwether was called pasture before. `./pasture` still works for one
  release (it prints a note and forwards to `./bellwether`), and `./bellwether migrate`
  moves an existing pasture install over (see "Coming from pasture").
- **Early, macOS-first:** it runs on macOS today. Windows and Linux (Hyprland) are
  planned, not built.
- **Credits:** the pets come from [desktopPet](https://github.com/Adrianotiger/desktopPet)
  by Adriano (MIT), and its eSheep, the inspiration and the pet format bellwether plays
  (`animations.xml`). See NOTICE.
- **No art in the repo:** bellwether downloads the sheep art on your machine (`./bellwether
  pets fetch`, pinned and sha256-checked). The art's own licence is unclear, so it is
  never committed or bundled.

It has two parts:

- the *brain*, a zero-dependency Node CLI: hook listener, state model, `/state`,
  `status`, `replay`, the hook installer and `doctor`, plus `pets fetch` and the
  brain's login autostart;
- the *shell* (`shell/`, Tauri 2): a transparent, click-through strip above the Dock
  with one coloured sheep per session, a bubble on the one that needs you, lambs for
  its subagents, and a menu-bar icon.

Nothing is installed into your `~/.claude/settings.json` or `~/Library/LaunchAgents`
until you run the `--write` steps in "First run".

## Two guarantees that it can't answer for you

1. **`/hook` never sends a body.** Claude Code treats a JSON body from an HTTP hook as
   a decision (approve a permission, block a tool call). `brain/hook-route.mjs` has one
   way to answer: an empty `204`, or an empty `403` when a request fails a guard. A
   source-scan test fails if that module could write a body.
2. **Every bellwether hook entry has `timeout: 1`.** A hung brain therefore stalls a tool
   call by one second at most (measured: exactly 1.000 s), not the 600 s default.
   `doctor` fails any entry without a timeout or above 2 s.

The listener binds `127.0.0.1` only; that is a constant, not configuration. Requests
with an `Origin` header, a foreign `Host`, a non-JSON `Content-Type` or the wrong
`X-Bellwether-Token` get `403`.

---

## First run: from nothing to sheep on screen

Each step says what you should see. `./bellwether doctor` ends with a `next`
line naming the step you're on, so you can stop and come back at any point. Run from
the repository root.

0. **Prerequisites:** Node 22.9 or newer (`node --version`), Rust (`cargo --version`;
   <https://rustup.rs>) and Xcode's command-line tools (`xcode-select --install`).
1. **A token**, into `.env` in the repository root (details in Setup step 1):
   ```
   ./bellwether token init
   ```
   It adds one only if there isn't one already, so it's safe to run twice. It fills
   an empty `BELLWETHER_HOOK_TOKEN=` line if there is one, never glues onto a last line
   without a newline, keeps the file's permissions, and never prints the token.
2. **Check:** `./bellwether doctor`. The token line passes. The brain line
   fails (nothing is running yet), and that's expected.
3. **The sheep art:** `./bellwether pets fetch`. It downloads the pinned
   file and checks its sha256; run again, it says `esheep64: already present and
   verified`.
4. **Build the strip, and try it:** `./bellwether shell --dev --release`. It
   compiles into `shell/src-tauri/target-dev/` (a few minutes the first time), then an
   empty strip opens above the Dock with a sheep in the menu bar. **Quit it** (menu-bar
   sheep > Quit Bellwether, or Ctrl-C). Then build the copy that starts at login:
   `./bellwether shell build --release` (into `shell/src-tauri/target/release/`).
5. **Start bellwether at login:**
   ```
   ./bellwether autostart install           # preview: the two LaunchAgents it would write
   ./bellwether autostart install --write   # writes and loads them: the brain and the strip start now, and at every login
   ```
   `./bellwether doctor` now shows the brain running and `autostart` passing.
6. **Install the hooks** (last, so a brain is always there to answer them):
   ```
   ./bellwether hooks install           # preview: the diff to ~/.claude/settings.json
   ./bellwether hooks install --write   # backs the file up to data/backups/, then writes
   ```
7. **Open a new Claude Code session.** A sheep walks into the strip. Ask it to run
   something that needs your permission: the sheep turns to face you with a bubble.
   `doctor`'s `next` line now says `all set`.

To undo everything: `./bellwether hooks uninstall --write`, then
`./bellwether autostart uninstall --write`.

---

## Setup

Needs Node 22.9 or newer (`node --version`). There is nothing to `npm install`.

### 1. Generate the hook token

```
./bellwether token init
```

That adds `BELLWETHER_HOOK_TOKEN=<a fresh random value>` to `.env` in the repository root
(created if missing; `.env.example` shows the format), unless it already has one,
reading the file the way Node does. `BELLWETHER_ENV=<file>` points bellwether at another
`.env`. If this repository sits at `agents/<name>` inside a larger hub and has no
`.env` of its own, the hub's `.env` two levels up is used. It never edits the file in place:
- it writes a new copy beside it (same permissions) and swaps it in;
- it keeps the previous contents in `.env.bak` (mode 0600, gitignored by `.env.*`);
- it follows a symlinked `.env`;
- it keeps CRLF line endings;
- it refuses a read-only file.

bellwether never prints the token: previews and logs show only its first four characters
and "…".

It's a local anti-spoofing value, not an account credential: it stops another app or
a browser tab from putting fake bubbles on your screen. Don't reuse an example value
from anywhere; bellwether refuses known example values. `hooks install` copies it
into the settings entries as a literal header value, so if you change it, re-run
`hooks install --write`.

### 2. Config (optional)

```
cp config.example.json config.json
```

Without `config.json` the example is used, and any key missing from `config.json`
takes its default. The brain reads `port` (default `47417`), `behaviour.*` (state
timings), `bubble.*` (detail, length, extra redaction patterns) and `pets.palette`
and `pets.maxLambsShown`. A `host` key is rejected: the bind address isn't
configurable. A `bubble.redactPatterns` entry that isn't a valid regular expression
is a config error (exit 3); `(?i)` at the start means case-insensitive.

### 3. Check it

```
./bellwether doctor
```

Expect two FAILs until you go further: `brain` (not running) and `hooks installed`.
Exit codes: `0` all good, `2` not running or not installed, `3` bad config or `.env`,
`1` anything else.

### 4. Try it without touching your settings

```
./bellwether hooks print --out data/hooks-only.json
./bellwether serve                  # terminal 1; one line per event
claude --settings data/hooks-only.json   # terminal 2
```

`--out` creates `data/` if it doesn't exist yet (it doesn't on a fresh clone) and
writes the file with mode 0600, because it contains your token. Relative paths are
relative to the directory you run `bellwether` from.

`--out` won't destroy anything by accident. It replaces an existing file only if
that file is already a bellwether hooks file; anything else needs `--force`. It refuses a
directory, and it always refuses a Claude Code settings file (`.claude/settings.json`,
`.claude/settings.local.json`, `managed-settings.json`), even with `--force`. Settings
are only ever changed by `hooks install`, which shows a diff and keeps a backup.
`record --out` has the same guard: it appends only to an empty file or an existing
bellwether recording.

Events show up in the `serve` log as you work (event, session, agent, tool name:
never a command or a message). In a third terminal, `./bellwether status
--watch` shows the flock: each session's state and, when it waits on you, its alert,
for example `ALERT  Bash: git push origin main?`. **Don't** leave a session running with
these hooks after you stop `serve`: with the brain down, every prompt and tool call
prints a visible `… hook error … ECONNREFUSED` line (measured).

### 5. Install the hooks

`./bellwether hooks install` prints the diff it *would* apply to
`~/.claude/settings.json` and writes nothing. It adds 16 matcher groups and removes
zero lines; other tools' hooks already in the file are untouched. Adding `--write` backs the file up to `data/backups/` first (mode 0600).

The installer keeps the file's own indentation, line endings and number literals, so
install followed by uninstall gives back the same bytes. If a file isn't laid out the
way bellwether writes JSON (for example an inline `[1, 2]` array), the preview says so
and shows the reformatted lines; values never change. `--settings` must be followed
by a path: a bare `--settings`, an unknown option (including odd names such as
`--__proto__`) or a stray argument is refused (exit 3) rather than falling back to
your real settings file.

Install autostart (step 7) **before** the hooks: with hooks installed and no brain
running, every session prints hook errors.

To remove them: `./bellwether hooks uninstall --write`.

### 6. Build and run the overlay

Needs Rust (`rustc --version`, 1.99 or newer) and Xcode's command-line tools. `bellwether`
finds cargo on PATH, in `~/.cargo/bin` or in `$CARGO_HOME/bin`; `doctor` shows which. Then:

```
./bellwether pets fetch              # the sheep art, pinned and sha256-checked, into data/pets/
./bellwether shell --dev --release   # builds into target-dev/ (about a minute the first time), then runs
./bellwether shell build --release   # builds the strip the login item runs, into target/release/
```

**Two build directories, on purpose.** macOS ties the Accessibility permission (sheep
standing on the Dock) to the exact binary the login item runs,
`shell/src-tauri/target/release/bellwether-shell`; rebuilding it drops the permission.
So:
- every development build goes to `shell/src-tauri/target-dev/`: `bellwether shell --dev`,
  `bellwether shell build` without `--release`, and plain `cargo build` / `cargo test`
  (with or without `--release`) run from the repository root, `shell/` or
  `shell/src-tauri`: a `.cargo/config.toml` in each says so. These never touch the
  login item's binary;
- only `bellwether shell build --release` writes `target/release/`. It says so before
  it builds. Afterwards: `bellwether shell restart`, then `bellwether shell
  dock-permission` to give Accessibility again.

Only `CARGO_TARGET_DIR` (which `shell build --release` sets) overrides that; don't set
it by hand in a checkout whose strip starts at login.

After the first build, `./bellwether shell` starts it straight away. It
runs without any macOS permission. Two are optional:
- *Accessibility*, so sheep can stand on the Dock itself: `./bellwether shell
  dock-permission`. The grant belongs to the built binary, so after every
  `shell build --release` it must be given again.
- *Automation*, for clicking a sheep into its exact iTerm2 or Terminal tab.

Screen Recording is never needed.

- It shows a strip along the bottom of the main screen, directly above the Dock, with
  one sheep per Claude Code session (start one with hooks, as in step 4). The strip is
  invisible except for the sheep.
- Started before there is a token, it waits: it looks for `BELLWETHER_HOOK_TOKEN` every
  3 s and connects once `bellwether token init` adds one (logged in `data/shell.log`).
- If the brain isn't running, the shell starts one, and stops it again when the shell
  quits: on Ctrl-C, `kill` (TERM, HUP) or a normal quit. If the shell is killed
  outright (`kill -9`), that brain notices within a second and stops itself. A brain
  you started yourself, or the login autostart's, is never stopped by the shell. Since
  milestone 5 of the distribution plan the brain it starts is the Rust one, from the
  shell's own binary (`bellwether-shell serve`), with this checkout's config, token and
  `data/`; Node isn't involved. The login autostart runs the shell with
  `--no-spawn-brain`, so it never starts one: its brain is the Node brain's LaunchAgent.
- Quit it with Ctrl-C in its terminal, or from its menu-bar icon.
- It loads only pinned pets whose file matches the sha256 in `pets.lock.json` (the
  same pins `pets fetch` uses). A changed or unknown pet file is refused with a line
  in `data/shell.log`; run `bellwether pets fetch` again.
- Its log is `data/shell.log`: one timestamped line per snapshot, sheep and click,
  never any bubble text.

What a sheep does:
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

**Colours:** each session's colour is on its sheep's **horns**. The wool, face and legs
stay as drawn. There are eight colours (`pets.palette`): orange, blue, black, red,
cyan, brown, green and pink, in the order sessions get them. To tint the whole sheep instead, as before, set
`"pets": {"tint": "whole"}` in `config.json`, then run `./bellwether shell
restart`. A `config.json` still carrying the old default palette (white first, then
pale blue) gets the new colours; doctor says so, and your file is left alone.

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
  it. To allow it, run `./bellwether shell dock-permission` and turn on
  bellwether-shell in System Settings > Privacy & Security > Accessibility, then
  `bellwether shell restart`.
- The grant belongs to the built binary, which is ad-hoc signed, so **after every
  rebuild it must be given again**. Until then, the full-width strip is used.
- **Granted before, and now `doctor` (or `data/shell.log`) says "not granted"?** The
  strip was rebuilt, and System Settings > Privacy & Security > Accessibility still
  shows the old bellwether-shell entry, switched on. That entry belongs to the previous
  binary, so switching it off and on does nothing. Remove the old `bellwether-shell`
  entry (select it, then −), run `./bellwether shell dock-permission`, turn the new
  entry on, and `bellwether shell restart`.

**Two sessions in one project** get told apart in their labels and bubbles:
`my-project · p2` and `my-project · p4` (the herdr pane). Otherwise it's the
iTerm tab (`tab 2`), the tmux pane, or a number (`#2`). A session alone in its project
keeps the plain name.

Debug flags on the shell binary (`shell/src-tauri/target/release/bellwether-shell`):

| Flag | Effect |
|---|---|
| `--demo` | one sheep per behaviour from a made-up flock; the brain isn't contacted |
| `--selftest` | parses every configured pet with the webview's real XML parser, prints what it found, exits 0 or 1 |
| `--snapshot <file.png> [--snapshot-after S] [--snapshot-every S]` | writes what the strip draws (plus a `-checker` copy that shows the transparency); `--snapshot-every` keeps writing `-t01`, `-t02`, … |
| `--quit-after S`, `--no-spawn-brain` | for scripted runs |

Set `BELLWETHER_SHELL_TRACE=1` to log hitboxes, every click-through switch, every
snapshot the webview fetches, and a once-a-second heartbeat with the page's
visibility. `bellwether replay <fixture> --tag <name>` plays a fixture as its own
session and sheep, so several fixtures can drive the strip at once. Each sent event
is printed with its time, for timing checks against `data/shell.log` (`alert shown`,
`alert confirmed` and `alert cleared` lines, no bubble text). These lines are written
when the bubble is drawn. New and confirmed alerts skip the brain's 250 ms snapshot
throttle; snapshots are still capped at 4 a second.

### 7. Start bellwether at login (macOS)

```
./bellwether autostart install          # shows the two LaunchAgents it would write
./bellwether autostart install --write  # writes and loads them (below)
./bellwether autostart status           # installed? loaded? which pid?
./bellwether autostart uninstall --write
```

Two LaunchAgents in `~/Library/LaunchAgents`, because the brain must keep answering
hooks even when the strip is quit or hidden:
- `local.bellwether.brain.plist`: the brain, restarted if it crashes.
- `local.bellwether.shell.plist`: the strip, the release build with
  `--no-spawn-brain`. It is restarted only if it *crashes*, so Quit in the menu bar
  keeps it quit until your next login. `./bellwether shell start` brings it
  back sooner, and `bellwether shell restart` restarts a running one. Both go through
  launchd.

`install` refuses if the strip isn't built yet. `--brain-only` installs (or removes)
just the brain.

- The brain runs with the `node` you ran the command with. Its log is `data/brain.log`;
  the strip's is `data/shell.log`.
- If you switch Node versions (nvm) or move the repository, run `install --write` again;
  `autostart status` says when the installed copy no longer matches.
- The token comes from the `.env` (Setup step 1) each time the brain starts; it is never written
  into the plist.

### Staying out of the way

- **Menu-bar icon** (a small sheep): *Mute (do not disturb)*, *Hide the strip*, *Show
  labels*, *Quit Bellwether*.
  - **Mute** is the same switch as `bellwether mute` / `bellwether unmute`, kept by the brain
    in `data/state.json`. The strip stops animating. Only a sheep (or lamb) that needs
    you shows a small "!", and clicks pass through the whole strip.
  - **Hide** and **Show labels** are kept in `data/shell-state.json`, so they survive
    a restart. A hidden strip comes back hidden.
  - **The way back without the tray:** `bellwether show` (and `bellwether hide`) do the same
    as the tray's Hide. A running shell follows within a second, and a stopped one
    starts that way. The shell log's "strip hidden" line says so too. If the tray
    can't be created while the strip is saved hidden, the shell shows the strip anyway
    and logs an ERROR line.
  - Autostart isn't in the menu; use `bellwether autostart`.
- **Screen sharing:**
  - The strip asks macOS to leave it out of screen captures (`privacy.captureExclusion`,
    `sharingType none`). Newer macOS capture APIs may ignore that, so it isn't relied on.
    The shell reads its config only when it starts: after changing `privacy.*` (or
    anything else in `config.json`), run `./bellwether shell restart`.
  - It also hides itself while a process named in `privacy.sharingProcesses` runs:
    Zoom's `CptHost`, which runs while Zoom shares. It comes back 2 s after that process
    exits.
  - Process names are compared whole, ignoring case, and macOS cuts them at 31 bytes.
    `bellwether doctor` and the shell log warn about any entry that can never match:
    empty, padded with spaces, a path, longer than 31 bytes, or a `.exe` name on a
    Mac.
  - Teams and Google Meet in a browser have no such process. Before sharing in those,
    use *Hide the strip*, or set `bubble.detail` to `"tool"` so a bubble never shows a
    command.
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
- **Click a sheep** to bring its terminal to the front.
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

### 8. Things to check by hand once

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
   1. *Mute*: the sheep vanish, and a waiting one shows only a "!". Run
      `bellwether status`: it shows MUTED. Quit and restart the shell: still muted.
      *Mute* again: back.
   2. *Hide the strip*: the strip disappears. Quit and restart the shell: it stays
      hidden. *Hide* again: back.
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

---

## Coming from pasture

Bellwether was called pasture. For one release the old names still work:
- **The command:** `./pasture` (and `pasture.cmd`) prints a note and forwards to
  `./bellwether`.
- **Variables:** each `PASTURE_*` variable is read when its `BELLWETHER_*` twin isn't
  set. The order is: an exported `BELLWETHER_*`, then `BELLWETHER_*` in `.env`, then an
  exported `PASTURE_*`, then `PASTURE_*` in `.env`. `doctor` lists the old names it used.
- **Hook entries:** the brain accepts `X-Pasture-*` headers. The entries pasture wrote
  are on the same URL, so `hooks install` replaces them and `hooks uninstall` removes
  them. `doctor` warns while they're there.
- **LaunchAgents:** `autostart install` replaces pasture's agents (found by what they
  run, whatever their label) with `local.bellwether.*`, previewed first; the old plists
  are backed up to `~/Library/Application Support/bellwether/launchagent-backups/`.

To move everything in one step, first quit any strip you started by hand (`bellwether
shell --dev --release`, say): menu-bar sheep > Quit Bellwether. Only one strip can run,
so a strip left running keeps the login item's from starting. `migrate` stops and says
so if it finds one, and the preview lists it.

```
./bellwether migrate --from <old pasture directory>           # preview
./bellwether migrate --from <old pasture directory> --write   # apply
```

`--from` is the old install's directory (with `brain/` and `data/`) or its data
directory; without it this checkout is upgraded in place. In this order:
1. the token: `PASTURE_HOOK_TOKEN` becomes `BELLWETHER_HOOK_TOKEN` with the same value,
   in this checkout's `.env` (`--env <file>` names the old `.env`);
2. `config.json`, if this checkout has none;
3. `data/state.json` (each session's colour and slot, and mute) and
   `data/shell-state.json`, if this checkout has none (`--force` replaces them);
4. pasture's LaunchAgents, found by what they run (any label), replaced by
   bellwether's:
   - first it checks who holds the brain's port. If it's anything but pasture's own
     brain, it stops and names the pid and command line; it never stops a process
     itself;
   - then it backs up the old plists to
     `~/Library/Application Support/bellwether/launchagent-backups/`, boots them
     out, and loads the new ones;
   - then it waits for the new brain to answer and accept the token. If it doesn't,
     the new agents are removed, the old ones are restored and loaded again, and the
     message lists what changed;
   - if bellwether's strip isn't built yet, pasture's strip agent is left as it is,
     and only the brain moves;
   - a strip that `bellwether shell start` started from this checkout is stopped
     first; any other strip started by hand stops the run, with how to quit it. After
     loading, it reports whether the login item's strip is running, with its pid;
5. only then, pasture's hook entries, replaced by bellwether's (a backup goes to
   `data/backups/`).

A settings file that can't be written is reported before anything changes. If a run
stops part-way, run the same command again: it finishes what is left.
The token is never shown, and nothing changes without `--write`.

## The signed app (in development)

The distribution plan in `docs/` turns Bellwether into one signed, notarized
`Bellwether.app`. Milestones 1 (a skeleton), 3 (the brain's state), 4 (the hook
installer, doctor, `pets import` and `migrate`) and 5 (the strip on the Rust brain, with
the app's own files) are in the repository. The app runs without Node. It doesn't replace
the setup above by itself: moving a repository install into the app is milestone 8, and
until then the plan's milestone 5 status lists the steps to do it by hand. Building it
never touches the repository strip or its LaunchAgents.

**The repository's strip is unchanged.** A strip built from this checkout (`bellwether
shell build --release`, `shell --dev`) is not inside an app bundle, so it keeps the
checkout's layout: `config.json`, the token from the environment or the `.env`, `data/`
and `data/shell.log`. Only `Bellwether.app` reads the app's files below.

**One executable, three modes.** `Bellwether.app/Contents/MacOS/Bellwether`:
- launched from Finder, it is the strip;
- `Bellwether serve` is the Rust brain. It never starts Tauri, windows or WebKit;
- called as `bellwether` (a lower-case symlink on PATH), or with a subcommand, it is
  the CLI: `serve`, `record`, `status [--watch] [--json]`, `mute`/`unmute`, `show`/`hide`,
  `replay`, `token init|rotate`, `hooks print|install|uninstall`, `doctor`, `pets import`,
  `migrate`, `autostart status|install|uninstall`, `version`, `help`. Those the Node CLI
  has take its flags and print its output (`test/DIVERGENCES.md` lists the exceptions).

Its files live in `~/Library/Application Support/Bellwether/`: `hook-token`, `config.json`
(optional; every key has a default), `state.json`, `shell-state.json`, `recordings/` and
imported pets (`pets/`, pinned in `pets.user.lock.json`). Logs go to
`~/Library/Logs/Bellwether/` (`brain.log`, `shell.log`). `BELLWETHER_HOME` overrides that
folder (logs then go to `<home>/logs/`), and `BELLWETHER_CONFIG` names another config file.

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
  own login agent (launchd lists that pid for the app's agent) running another version
  (after an update), it restarts the agent (`launchctl kickstart -k`) and waits for the
  new version. Any other brain is left alone,
  with a line in `shell.log`;
- loads a pet bundled with the app (none until milestone 10), else one you imported with
  `bellwether pets import`, each only while it matches its pin. `pets.default` in
  `config.json` names it. Until the app has its own sheep, import one and set it:

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

The installer and doctor work as the Node CLI's do (`hooks install` previews are the same
bytes), with the app's files in place of `data/`: the install record is `installs.json`
and backups go to `backups/` in the app's folder. Doctor also checks the token file's mode,
the app's identity and signature, its login items and Accessibility, LaunchAgents left by a
repository install, whether the strip could switch WebKit's occlusion detection off, and
fails a bellwether hook entry that sets `"onFailure": "block"`.

```
bellwether pets import ~/Downloads/mypet/animations.xml   # check, copy and pin a desktopPet file (16 MiB, UTF-8, no DOCTYPE, tiles up to 64)
bellwether migrate --from <repository>                    # preview: what a repository install's token, config, colours and hooks would become
bellwether migrate --from <repository> --write            # do it; its LaunchAgents are listed and left alone (the app's swap is milestone 8)
```

**What you need once:**
1. The **Developer ID Application** certificate, with its private key, in your login
   keychain. `security find-identity -v -p codesigning` lists it.
2. The first time `codesign` uses that key, macOS asks whether `codesign` may use it.
   Choose **Always Allow** (it asks for your login keychain password). Until you do,
   every build stops at signing.
3. A notarytool keychain profile named `bellwether`, made from an App Store Connect
   API key (Users and Access > Integrations; the `.p8` downloads only once):
   `xcrun notarytool store-credentials bellwether --key <AuthKey.p8> --key-id <key id> --issuer <issuer id>`.
4. The Tauri CLI, into `shell/node_modules` (gitignored):
   `npm install --prefix shell --no-package-lock`.

**Build:**

```
node scripts/build-app.mjs --notarize        # build, sign, notarize, staple, then check with spctl
node scripts/build-app.mjs                   # build and sign only
node scripts/build-app.mjs --adhoc           # no keychain: ad hoc, hardened, under its own id (…bellwether.adhoc)
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
npm run test:rust                            # the parity table: the brain's tests against both brains (see Tests)
```

`autostart` registers the brain's login agent, which is the plist inside the bundle at
`Contents/Library/LaunchAgents/`, and the app itself as a login item. It never writes
to `~/Library/LaunchAgents`. `--brain-only` leaves the app login item alone, and
`--via-app` hands the call to the app instead. It refuses an ad-hoc build carrying the
permanent id: macOS would keep that registration's launch constraint and then kill the
Developer ID build's brain ("Launch Constraint Violation"). If that happens anyway,
`autostart uninstall --write`, then `install --write` again.

More about `autostart`:
- A test build (`--test-port`/`--test-home`) may only use `--brain-only`: a test build's
  strip never starts at login. (Before milestone 5 the reason was that such a strip would
  use the live defaults, port 47417; it now takes the bundle's test port and home.)
- `uninstall --write` also stops this bundle's running strip.
- In a terminal opened before your last login (a herdr or tmux server that outlived a
  logout), macOS won't answer about login items. `autostart` and `doctor` then say so:
  open a new terminal window.

## Licensing

MIT (`LICENSE`), for this repository's code. `NOTICE` covers:
- the pet format (desktopPet, MIT, credited);
- the pet art, which is fetched by `pets fetch` and never committed or bundled,
  because its licence is unclear;
- web-esheep, GPL-3.0, not vendored (a reference only);
- the Linux library for later.

`THIRD-PARTY.md` lists every Rust crate and npm package with its licence. Regenerate it
with `node scripts/third-party.mjs` after changing a dependency (offline;
`--check` says when it's stale).

## Commands

| Command | What it does |
|---|---|
| `bellwether serve [--hang \| --slow=<ms>]` | Runs the listener in the foreground. `--hang` (accept, never answer) and `--slow` are debug flags for timeout measurements |
| `bellwether record [--out <file>]` | `serve`, plus one line per event in `data/recordings/<ts>.jsonl`: the normalised event, the payload's **field names** (never values) and which terminal headers arrived. `--out` appends only to an empty file or an existing bellwether recording |
| `bellwether status [--watch] [--json]` | The flock as a table: label, colour, state, alert, lambs. `--watch` redraws on every change (long-polls `/state`); this is also the terminal fallback when there's no overlay |
| `bellwether replay <file.jsonl> [--speed N] [--port P]` | Posts a recording or fixture to a running brain with its original timing, scaled by `--speed`, and prints `/hook` latency (p50, p99, max) and the snapshot rate. Payloads are rebuilt with the real Claude Code field names |
| `bellwether replay --burst <events/s> [--seconds S]` | A synthetic load of tool events, for the rate and latency checks |
| `bellwether doctor [--json] [--e2e] [--settings <path>]` | Node, config, token, brain running and accepting the token, hooks installed in the settings file (timeout, token, no `"onFailure": "block"`, `disableAllHooks`, `allowedHttpHookUrls`), and the project, local and managed settings levels that can switch hooks off. `--e2e` also runs one `claude -p` with haiku and bellwether's hooks from a throwaway file (one small API call; your user settings and MCP servers are left out) and checks the events arrive |
| `bellwether shell [--dev] [--release]` | Runs the overlay in this terminal; `--dev` builds it with cargo first |
| `bellwether shell build [--release]` | Builds the strip. Without `--release`: the debug build, into `target-dev/`. With it: the binary the login item runs, in `target/release/`; it warns first, because the Accessibility permission has to be given again afterwards |
| `bellwether shell dock-permission` | Asks macOS for Accessibility for the strip's release binary (shows the system prompt), so sheep can stand on the Dock. Only when you run it |
| `bellwether shell start` / `stop` / `restart` | The strip in the background. With its LaunchAgent installed: `launchctl kickstart` / `kill TERM` / `kickstart -k`. Otherwise bellwether runs the release build itself (pid in `data/shell.pid`), and stops only that exact binary. Use `restart` after a config change |
| `bellwether mute` / `bellwether unmute` | Do-not-disturb through the brain, the same as the tray's Mute; kept across restarts |
| `bellwether show` / `bellwether hide` | Show or hide the strip, the same as the tray's Hide; a running shell follows within a second; kept across restarts |
| `bellwether autostart install\|uninstall [--write] [--brain-only]`, `autostart status` | The brain's and the strip's LaunchAgents (macOS). Preview by default |
| `bellwether pets fetch [<pet>]` | Downloads the pinned pet art into `data/pets/` and checks its sha256. The art is never committed |
| `bellwether migrate [--from <dir>] [--env <file>] [--settings <path>] [--force] [--write]` | Moves a pasture install over (token, config, colours and slots, hook entries, LaunchAgents). Preview unless `--write`; see "Coming from pasture" |
| `bellwether hooks print [--out <file> [--force]]` | The hook block as a standalone settings file, to stdout or (0600) to a file. Replaces only an earlier bellwether hooks file unless `--force`; never a Claude Code settings file |
| `bellwether hooks install [--write] [--settings <path>]` | Dry-run diff by default. Idempotent: a second run says "already installed, no changes" |
| `bellwether hooks uninstall [--write] [--settings <path>] [--port <old>]` | Removes exactly the entries whose URL is `http://127.0.0.1:<port>/hook` (exact match: a `…/hooks` or `…/hook2` belonging to something else is never touched). After a port change, the old entries stay until you run it with `--port <old port>`; `doctor` points them out. Event keys that existed before the install (even empty ones) stay; `data/installs.json` remembers which ones the install created. That record is advisory: if it is missing or corrupt you get a warning, and uninstall removes the event keys bellwether emptied |

`npm run doctor` (and `serve`, `record`, `hooks`, `status`, `replay`) works too.
Windows (not built yet): `bellwether.cmd`. A symlink to `bellwether` on your PATH works.

## What the brain shows, and what it keeps

- **One sheep per terminal pane or tab**, keyed innermost first: the herdr pane, then
  the tmux pane, then the iTerm2 tab, then the Terminal.app tab, then the Windows
  Terminal tab, and only without any of those the session. So `/clear`, which starts
  a new session, keeps the same sheep, colour and slot. Colours and slots persist in
  `data/state.json` across brain restarts.
- **Lambs** are subagents (Explore, code-reviewer, …). Claude Code's internal helper agent,
  which reports a nameless `SubagentStop` after most turns, never becomes a lamb.
- **States:** arriving, busy (running, digging, reading, thinking), done, idle,
  waiting, sleeping, compacting, error, leaving. Each lasts at least
  `behaviour.minDwellMs`, so a burst of tool calls doesn't make a sheep flicker.
- **Alerts** (permission, elicitation, input) appear at once and stay until Claude Code
  shows the prompt was answered. There is no timeout. A lamb's alert survives the main
  thread's turn ending; only its own tool call, its `SubagentStop` or the session
  ending clears it.
- **Text:** a bubble shows the tool and one allowlisted field (the Bash command, a
  file's name, a URL's host, a search query or pattern, the subagent type) or the
  notification's message. It is redacted and cut to `bubble.maxChars`. Redaction
  covers anything named like a credential (`GITHUB_TOKEN=`, `"password":`,
  `--password x`, `?access_token=`, quoted values with spaces), Authorization headers,
  `curl -u user:pass`, `mysql -pSECRET`, URL credentials (everything after the user name up
  to the last `@` before the host, so a password containing `@` or `:` is hidden whole;
  an `@` in the path or query is left alone), common token formats, and
  your `bubble.redactPatterns`. Only the secret becomes `•••`; the name stays, as in
  `DB_PASSWORD=•••`. Control and bidi characters are stripped. `bubble.detail:
  "tool"` shows only `Bash?`. Recordings never contain this text, and replaying a real
  recording (field names only) shows tool-name bubbles such as `Bash?`.
- **Nothing lingers or grows without bound:** a lamb whose `SubagentStop` never came
  leaves after `behaviour.lambTimeoutMin` (unless it holds an alert), a tool whose
  `PostToolUse` never came stops counting after `behaviour.toolTimeoutMin`, and
  `data/state.json` is checked on load and capped (identities, monitors per sheep,
  lambs per sheep).

## Tests

```
npm test               # the JS suite
npm run test:cargo     # the Rust tests (shell and brain), from the repository root
npm run test:rust      # the parity table: the brain's tests against the Node and the Rust brain
npm run test:vectors   # the shared vector files still match what the Node brain answers
npm run differential   # Node and Rust on 10^5 generated cases per area
```

`cargo test` on its own has to be run in `shell/src-tauri`, where the Cargo workspace
is; from the root, use `npm run test:cargo`.

**The Rust port's conformance harness** (distribution plan D2). The Node brain is the
oracle until the Rust brain replaces it:

- Every test that starts the brain or its CLI goes through `spawnBrain` in
  `test/helpers.mjs`. `BELLWETHER_IMPL=rust` runs `$BELLWETHER_BIN` instead of
  `brain/cli.mjs`; by default that is the dev build
  `shell/src-tauri/target-dev/debug/bellwether-shell`, which must not be older than its
  sources. Under rust the helper makes the test's data dir the binary's
  `BELLWETHER_HOME` and passes the test's config as `BELLWETHER_CONFIG`, refuses anything
  that would use port 47417, and never runs `autostart` (it would register login items on
  this Mac).
- Tests that check the brain's parts in process take them from `test/impl.mjs`: brain/*.mjs
  under node, and under rust the Rust brain, called synchronously through its hidden
  `__session` command (one call per line, over two FIFOs); `startBrain` is then a Rust
  `serve` process; `runDoctor` is a `doctor --json` run of the binary. A test of something
  only the repository has (the Node shim, the `.env` writer, LaunchAgent `autostart` and
  `shell start|stop`, `pets fetch`) is marked `notUnderRust(reason)` and counts as skipped
  under rust, never as passed. Under rust the helper also gives the binary a temp HOME
  whenever a test gives none or the real one, and the preload refuses any read or write of
  the real `~/.claude/settings.json` in every Node test process.
- `npm run test:rust` (`test/conformance.mjs`) runs those test files against both brains
  and prints the parity table: test file, node, rust. `test/conformance.json` says which
  milestone turns each row green; the run fails only when a node cell is red or a row the
  current milestone requires is red under rust. Build the binary first, with the hidden
  commands the harness uses:

  ```
  cargo build --manifest-path shell/src-tauri/Cargo.toml --features bellwether-shell/conformance
  ```

- The `conformance` feature adds hidden `__` commands that take cases as JSON lines:
  `__env`, `__redact`, `__normalise`, `__fixture`, `__json` (a settings file through the
  lossless JSON and back) and `__diff`, and `__session` (the bridge above). It also lets `serve` read test timings
  from the environment. No release build has it: `Bellwether __fixture` answers "unknown
  command" with exit 1.
- The shared vectors in `test/fixtures/` (`redact-`, `normalise-`, `config-` and
  `diff-vectors.json`, `installer-vectors/`) hold cases and the Node brain's answers. The
  Node tests and the cargo tests read the same files. `node test/fixtures/build-vectors.mjs`
  rewrites them; read the diff before committing it.
- `npm run differential` (`scripts/differential.mjs`) sends generated inputs to the Node
  functions and the Rust binary and fails on any difference. Areas the binary doesn't
  have yet are listed as not ported.
- Intended differences between the two brains are in `test/DIVERGENCES.md`, each with
  the test that pins it. Anything else is a bug.

**CI** (`.github/workflows/ci.yml`, GitHub Actions, no secrets): the Node suite and the
vector check, the cargo tests with and without the `conformance` feature, the parity table
(published as the job summary) and the differential, a release build's refusal of
`__fixture`, and `cargo check` of the platform-neutral crates (`crates/core`,
`crates/brain`) for Linux and Windows. Before the suite, CI does what a developer's
checkout already has: `cargo fetch` (every
platform's crates, whose licences THIRD-PARTY.md lists), a debug build of the strip, and
`pets fetch` (the pinned, hash-checked art; never committed). The redaction time budgets
are written for a developer's Mac and scale with the machine's speed, measured in the same
process (`machineSlowdown` in `test/helpers.mjs`).

Runs serially (`--test-concurrency=1`): several suites use live sockets and timing.
Offline and self-contained. The installer tests only ever use temp copies of an
invented settings file shaped like yours; nothing reads or writes
`~/.claude/settings.json`.
