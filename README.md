# Bellwether

A desktop-pet strip that shows each of your Claude Code sessions as a sheep along the
bottom of the screen. The sheep for a session that is waiting on you (a permission
prompt, a question) turns to face you with a bubble saying what it wants; click it to
jump to that terminal.

- **The name:** the project is Bellwether. The command is still `pasture` while the
  rename is in progress.
- **Early, macOS-first:** it runs on macOS today. Windows and Linux (Hyprland) are
  planned, not built.
- **Credits:** the pets come from [desktopPet](https://github.com/Adrianotiger/desktopPet)
  by Adriano (MIT), and its eSheep, the inspiration and the pet format pasture plays
  (`animations.xml`). See NOTICE.
- **No art in the repo:** pasture downloads the sheep art on your machine (`./pasture
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
2. **Every pasture hook entry has `timeout: 1`.** A hung brain therefore stalls a tool
   call by one second at most (measured: exactly 1.000 s), not the 600 s default.
   `doctor` fails any entry without a timeout or above 2 s.

The listener binds `127.0.0.1` only; that is a constant, not configuration. Requests
with an `Origin` header, a foreign `Host`, a non-JSON `Content-Type` or the wrong
`X-Pasture-Token` get `403`.

---

## First run: from nothing to sheep on screen

Each step says what you should see. `./pasture doctor` ends with a `next`
line naming the step you're on, so you can stop and come back at any point. Run from
the repository root.

0. **Prerequisites:** Node 22.9 or newer (`node --version`), Rust (`cargo --version`;
   <https://rustup.rs>) and Xcode's command-line tools (`xcode-select --install`).
1. **A token**, into `.env` in the repository root (details in Setup step 1):
   ```
   ./pasture token init
   ```
   It adds one only if there isn't one already, so it's safe to run twice. It fills
   an empty `PASTURE_HOOK_TOKEN=` line if there is one, never glues onto a last line
   without a newline, keeps the file's permissions, and never prints the token.
2. **Check:** `./pasture doctor`. The token line passes. The brain line
   fails (nothing is running yet), and that's expected.
3. **The sheep art:** `./pasture pets fetch`. It downloads the pinned
   file and checks its sha256; run again, it says `esheep64: already present and
   verified`.
4. **Build the strip, and try it:** `./pasture shell --dev --release`. It
   compiles (a few minutes the first time), then an empty strip opens above the Dock
   with a sheep in the menu bar. **Quit it** (menu-bar sheep > Quit pasture, or Ctrl-C):
   the next step starts it for good.
5. **Start pasture at login:**
   ```
   ./pasture autostart install           # preview: the two LaunchAgents it would write
   ./pasture autostart install --write   # writes and loads them: the brain and the strip start now, and at every login
   ```
   `./pasture doctor` now shows the brain running and `autostart` passing.
6. **Install the hooks** (last, so a brain is always there to answer them):
   ```
   ./pasture hooks install           # preview: the diff to ~/.claude/settings.json
   ./pasture hooks install --write   # backs the file up to data/backups/, then writes
   ```
7. **Open a new Claude Code session.** A sheep walks into the strip. Ask it to run
   something that needs your permission: the sheep turns to face you with a bubble.
   `doctor`'s `next` line now says `all set`.

To undo everything: `./pasture hooks uninstall --write`, then
`./pasture autostart uninstall --write`.

---

## Setup

Needs Node 22.9 or newer (`node --version`). There is nothing to `npm install`.

### 1. Generate the hook token

```
./pasture token init
```

That adds `PASTURE_HOOK_TOKEN=<a fresh random value>` to `.env` in the repository root
(created if missing; `.env.example` shows the format), unless it already has one,
reading the file the way Node does. `PASTURE_ENV=<file>` points pasture at another
`.env`. If this repository sits at `agents/<name>` inside a larger hub and has no
`.env` of its own, the hub's `.env` two levels up is used. It never edits the file in place:
- it writes a new copy beside it (same permissions) and swaps it in;
- it keeps the previous contents in `.env.bak` (mode 0600, gitignored by `.env.*`);
- it follows a symlinked `.env`;
- it keeps CRLF line endings;
- it refuses a read-only file.

pasture never prints the token: previews and logs show only its first four characters
and "…".

It's a local anti-spoofing value, not an account credential: it stops another app or
a browser tab from putting fake bubbles on your screen. Don't reuse an example value
from anywhere; pasture refuses known example values. `hooks install` copies it
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
./pasture doctor
```

Expect two FAILs until you go further: `brain` (not running) and `hooks installed`.
Exit codes: `0` all good, `2` not running or not installed, `3` bad config or `.env`,
`1` anything else.

### 4. Try it without touching your settings

```
./pasture hooks print --out data/hooks-only.json
./pasture serve                  # terminal 1; one line per event
claude --settings data/hooks-only.json   # terminal 2
```

`--out` creates `data/` if it doesn't exist yet (it doesn't on a fresh clone) and
writes the file with mode 0600, because it contains your token. Relative paths are
relative to the directory you run `pasture` from.

`--out` won't destroy anything by accident. It replaces an existing file only if
that file is already a pasture hooks file; anything else needs `--force`. It refuses a
directory, and it always refuses a Claude Code settings file (`.claude/settings.json`,
`.claude/settings.local.json`, `managed-settings.json`), even with `--force`. Settings
are only ever changed by `hooks install`, which shows a diff and keeps a backup.
`record --out` has the same guard: it appends only to an empty file or an existing
pasture recording.

Events show up in the `serve` log as you work (event, session, agent, tool name:
never a command or a message). In a third terminal, `./pasture status
--watch` shows the flock: each session's state and, when it waits on you, its alert,
for example `ALERT  Bash: git push origin main?`. **Don't** leave a session running with
these hooks after you stop `serve`: with the brain down, every prompt and tool call
prints a visible `… hook error … ECONNREFUSED` line (measured).

### 5. Install the hooks

`./pasture hooks install` prints the diff it *would* apply to
`~/.claude/settings.json` and writes nothing. It adds 16 matcher groups and removes
zero lines; other tools' hooks already in the file are untouched. Adding `--write` backs the file up to `data/backups/` first (mode 0600).

The installer keeps the file's own indentation, line endings and number literals, so
install followed by uninstall gives back the same bytes. If a file isn't laid out the
way pasture writes JSON (for example an inline `[1, 2]` array), the preview says so
and shows the reformatted lines; values never change. `--settings` must be followed
by a path: a bare `--settings`, an unknown option (including odd names such as
`--__proto__`) or a stray argument is refused (exit 3) rather than falling back to
your real settings file.

Install autostart (step 7) **before** the hooks: with hooks installed and no brain
running, every session prints hook errors.

To remove them: `./pasture hooks uninstall --write`.

### 6. Build and run the overlay

Needs Rust (`rustc --version`, 1.99 or newer) and Xcode's command-line tools. Then:

```
./pasture pets fetch          # the sheep art, pinned and sha256-checked, into data/pets/
./pasture shell --dev --release   # builds with cargo (about a minute the first time), then runs
```

After the first build, `./pasture shell` starts it straight away. It
runs without any macOS permission. Two are optional:
- *Accessibility*, so sheep can stand on the Dock itself: `./pasture shell
  dock-permission`. The grant belongs to the built binary, so after every rebuild it
  must be given again.
- *Automation*, for clicking a sheep into its exact iTerm2 or Terminal tab.

Screen Recording is never needed.

- It shows a strip along the bottom of the main screen, directly above the Dock, with
  one sheep per Claude Code session (start one with hooks, as in step 4). The strip is
  invisible except for the sheep.
- If the brain isn't running, the shell starts it, and stops it again when the shell
  quits: on Ctrl-C, `kill` (TERM, HUP) or a normal quit. If the shell is killed
  outright (`kill -9`), that brain notices within a second and stops itself. A brain
  you started yourself, or the login autostart's, is never stopped by the shell.
- Quit it with Ctrl-C in its terminal, or from its menu-bar icon.
- It loads only pinned pets whose file matches the sha256 in `pets.lock.json` (the
  same pins `pets fetch` uses). A changed or unknown pet file is refused with a line
  in `data/shell.log`; run `pasture pets fetch` again.
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
`"pets": {"tint": "whole"}` in `config.json`, then run `./pasture shell
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
  it. To allow it, run `./pasture shell dock-permission` and turn on
  pasture-shell in System Settings > Privacy & Security > Accessibility, then
  `pasture shell restart`.
- The grant belongs to the built binary, which is ad-hoc signed, so **after every
  rebuild it must be given again**. Until then, the full-width strip is used.

**Two sessions in one project** get told apart in their labels and bubbles:
`my-project · p2` and `my-project · p4` (the herdr pane). Otherwise it's the
iTerm tab (`tab 2`), the tmux pane, or a number (`#2`). A session alone in its project
keeps the plain name.

Debug flags on the shell binary (`shell/src-tauri/target/release/pasture-shell`):

| Flag | Effect |
|---|---|
| `--demo` | one sheep per behaviour from a made-up flock; the brain isn't contacted |
| `--selftest` | parses every configured pet with the webview's real XML parser, prints what it found, exits 0 or 1 |
| `--snapshot <file.png> [--snapshot-after S] [--snapshot-every S]` | writes what the strip draws (plus a `-checker` copy that shows the transparency); `--snapshot-every` keeps writing `-t01`, `-t02`, … |
| `--quit-after S`, `--no-spawn-brain` | for scripted runs |

Set `PASTURE_SHELL_TRACE=1` to log hitboxes, every click-through switch, every
snapshot the webview fetches, and a once-a-second heartbeat with the page's
visibility. `pasture replay <fixture> --tag <name>` plays a fixture as its own
session and sheep, so several fixtures can drive the strip at once. Each sent event
is printed with its time, for timing checks against `data/shell.log` (`alert shown`,
`alert confirmed` and `alert cleared` lines, no bubble text). These lines are written
when the bubble is drawn. New and confirmed alerts skip the brain's 250 ms snapshot
throttle; snapshots are still capped at 4 a second.

### 7. Start pasture at login (macOS)

```
./pasture autostart install          # shows the two LaunchAgents it would write
./pasture autostart install --write  # writes and loads them (below)
./pasture autostart status           # installed? loaded? which pid?
./pasture autostart uninstall --write
```

Two LaunchAgents in `~/Library/LaunchAgents`, because the brain must keep answering
hooks even when the strip is quit or hidden:
- `local.pasture.brain.plist`: the brain, restarted if it crashes.
- `local.pasture.shell.plist`: the strip, the release build with
  `--no-spawn-brain`. It is restarted only if it *crashes*, so Quit in the menu bar
  keeps it quit until your next login. `./pasture shell start` brings it
  back sooner, and `pasture shell restart` restarts a running one. Both go through
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
  labels*, *Quit pasture*.
  - **Mute** is the same switch as `pasture mute` / `pasture unmute`, kept by the brain
    in `data/state.json`. The strip stops animating. Only a sheep (or lamb) that needs
    you shows a small "!", and clicks pass through the whole strip.
  - **Hide** and **Show labels** are kept in `data/shell-state.json`, so they survive
    a restart. A hidden strip comes back hidden.
  - **The way back without the tray:** `pasture show` (and `pasture hide`) do the same
    as the tray's Hide. A running shell follows within a second, and a stopped one
    starts that way. The shell log's "strip hidden" line says so too. If the tray
    can't be created while the strip is saved hidden, the shell shows the strip anyway
    and logs an ERROR line.
  - Autostart isn't in the menu; use `pasture autostart`.
- **Screen sharing:**
  - The strip asks macOS to leave it out of screen captures (`privacy.captureExclusion`,
    `sharingType none`). Newer macOS capture APIs may ignore that, so it isn't relied on.
    The shell reads its config only when it starts: after changing `privacy.*` (or
    anything else in `config.json`), run `./pasture shell restart`.
  - It also hides itself while a process named in `privacy.sharingProcesses` runs:
    Zoom's `CptHost`, which runs while Zoom shares. It comes back 2 s after that process
    exits.
  - Process names are compared whole, ignoring case, and macOS cuts them at 31 bytes.
    `pasture doctor` and the shell log warn about any entry that can never match:
    empty, padded with spaces, a path, longer than 31 bytes, or a `.exe` name on a
    Mac.
  - Teams and Google Meet in a browser have no such process. Before sharing in those,
    use *Hide the strip*, or set `bubble.detail` to `"tool"` so a bubble never shows a
    command.
- **Full-screen apps:** the strip stays off a full-screen app's Space.
- **Near the cursor:** a sheep (and its bubble) fades to `fade.opacity` while the
  cursor is within `fade.radiusPx` of it, so you can see and click what's underneath.
- `PASTURE_CONFIG=<file>` points the shell at another config file, read-only. It's a
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
    and a click brings the app forward. `pasture doctor` then names the missing
    permission.
  - `"locator": {"appleScript": false}` turns AppleScript off entirely.
- `locator.strategies` sets the order of the lookups, and `locator.herdrPath` sets
  where herdr is (default `herdr`, also found in `~/.local/bin`).
- `pasture-shell --locate '<terminal json>'` (a debug flag) prints what one lookup
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
      `pasture status`: it shows MUTED. Quit and restart the shell: still muted.
      *Mute* again: back.
   2. *Hide the strip*: the strip disappears. Quit and restart the shell: it stays
      hidden. *Hide* again: back.
   3. *Show labels*: every sheep shows its project.
   4. *Quit pasture*: the shell quits. A brain started by the shell stops too.
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
    3. `pasture doctor` shows an `automation` warning naming iTerm2.
15. **A missing herdr CLI.** Set `"locator": {"herdrPath": "/nope/herdr"}` in
    `config.json` and restart the shell. herdr sessions are still placed (through
    iTerm2 or the process tree), and `pasture doctor` shows `herdr CLI`. Remove the
    setting afterwards.

---

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
| `pasture serve [--hang \| --slow=<ms>]` | Runs the listener in the foreground. `--hang` (accept, never answer) and `--slow` are debug flags for timeout measurements |
| `pasture record [--out <file>]` | `serve`, plus one line per event in `data/recordings/<ts>.jsonl`: the normalised event, the payload's **field names** (never values) and which terminal headers arrived. `--out` appends only to an empty file or an existing pasture recording |
| `pasture status [--watch] [--json]` | The flock as a table: label, colour, state, alert, lambs. `--watch` redraws on every change (long-polls `/state`); this is also the terminal fallback when there's no overlay |
| `pasture replay <file.jsonl> [--speed N] [--port P]` | Posts a recording or fixture to a running brain with its original timing, scaled by `--speed`, and prints `/hook` latency (p50, p99, max) and the snapshot rate. Payloads are rebuilt with the real Claude Code field names |
| `pasture replay --burst <events/s> [--seconds S]` | A synthetic load of tool events, for the rate and latency checks |
| `pasture doctor [--json] [--e2e] [--settings <path>]` | Node, config, token, brain running and accepting the token, hooks installed in the settings file (timeout, token, `disableAllHooks`, `allowedHttpHookUrls`), and the project, local and managed settings levels that can switch hooks off. `--e2e` also runs one `claude -p` with haiku and pasture's hooks from a throwaway file (one small API call; your user settings and MCP servers are left out) and checks the events arrive |
| `pasture shell [--dev] [--release]` | Runs the overlay in this terminal; `--dev` builds it with cargo first |
| `pasture shell dock-permission` | Asks macOS for Accessibility for the strip's release binary (shows the system prompt), so sheep can stand on the Dock. Only when you run it |
| `pasture shell start` / `stop` / `restart` | The strip in the background. With its LaunchAgent installed: `launchctl kickstart` / `kill TERM` / `kickstart -k`. Otherwise pasture runs the release build itself (pid in `data/shell.pid`), and stops only that exact binary. Use `restart` after a config change |
| `pasture mute` / `pasture unmute` | Do-not-disturb through the brain, the same as the tray's Mute; kept across restarts |
| `pasture show` / `pasture hide` | Show or hide the strip, the same as the tray's Hide; a running shell follows within a second; kept across restarts |
| `pasture autostart install\|uninstall [--write] [--brain-only]`, `autostart status` | The brain's and the strip's LaunchAgents (macOS). Preview by default |
| `pasture pets fetch [<pet>]` | Downloads the pinned pet art into `data/pets/` and checks its sha256. The art is never committed |
| `pasture hooks print [--out <file> [--force]]` | The hook block as a standalone settings file, to stdout or (0600) to a file. Replaces only an earlier pasture hooks file unless `--force`; never a Claude Code settings file |
| `pasture hooks install [--write] [--settings <path>]` | Dry-run diff by default. Idempotent: a second run says "already installed, no changes" |
| `pasture hooks uninstall [--write] [--settings <path>] [--port <old>]` | Removes exactly the entries whose URL is `http://127.0.0.1:<port>/hook` (exact match: a `…/hooks` or `…/hook2` belonging to something else is never touched). After a port change, the old entries stay until you run it with `--port <old port>`; `doctor` points them out. Event keys that existed before the install (even empty ones) stay; `data/installs.json` remembers which ones the install created. That record is advisory: if it is missing or corrupt you get a warning, and uninstall removes the event keys pasture emptied |

`npm run doctor` (and `serve`, `record`, `hooks`, `status`, `replay`) works too.
Windows (not built yet): `pasture.cmd`. A symlink to `pasture` on your PATH works.

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
  `curl -u user:pass`, `mysql -pSECRET`, URL credentials, common token formats, and
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
npm test
```

Runs serially (`--test-concurrency=1`): several suites use live sockets and timing.
Offline and self-contained. The installer tests only ever use temp copies of an
invented settings file shaped like yours; nothing reads or writes
`~/.claude/settings.json`.
