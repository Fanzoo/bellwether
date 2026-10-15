# Bellwether: distribution plan

A plan for turning Bellwether from a repository you clone and build into a signed,
notarized, auto-updating macOS app. Windows and Linux stay possible later.

Status: proposal. Nothing in it is built yet. Items marked **ASSUMPTION** are guesses
the maintainer should confirm or correct before development starts. Bracketed numbers
such as [4] point to the Sources list at the end. Version numbers were checked against
crates.io on 2026-10-10 [27].

---

## Goal

Today Bellwether needs a clone, Node 22.9+, Rust, Xcode's command-line tools, a
hand-run token step, hand-written LaunchAgents, and an Accessibility grant that is lost
on every rebuild. After this plan ships, someone can download `Bellwether.dmg` (or run
`brew install --cask bellwether`) and drag it to Applications. A first-run window then
installs the hooks, with the same preview-then-confirm the CLI has, turns on start at
login, and offers the optional Dock and click-to-focus permissions. The app ships its
own original sheep and updates itself. No Node runtime is needed. Someone already
running the repository version moves over in one guided step and keeps their sessions'
colours, slots and token.

## Non-goals

- **Mac App Store.** The sandbox would break process listing, Dock Accessibility,
  AppleScript to terminals and the private WebKit call. Distribution is Developer ID
  only.
- **Windows and Linux builds.** They are out of scope here. The only requirement is
  that nothing in the brain or the CLI depends on macOS (checked in CI, M2).
- **New features.** No new pet behaviours, terminals, states or bubble kinds. The flock
  model, the redaction rules and the HTTP contracts are ported as they are.
- **Changing the webview player.** `shell/src/player/*.mjs` stays JavaScript and keeps
  its `node --test` suite.
- **Keychain storage of the hook token.** Considered and rejected; see D3.
- **A settings GUI beyond onboarding.** The onboarding window can be reopened from the
  tray ("Set up Bellwether…") to rerun its steps. `config.json` stays the way to tune
  timings and palettes.
- **Telemetry, crash reporting, accounts.** The app makes no network calls except the
  update check (and, optionally, the user's own `pets fetch`; see open question Q6).
- **Redistributing the eSheep art.** It stays out of the bundle.

## Surface

One executable, `Bellwether.app/Contents/MacOS/Bellwether`. It runs in three modes,
chosen by its arguments:

| Invocation | Mode | What it does |
|---|---|---|
| Launched from Finder or Login Items (no subcommand, `argv[0]` basename `Bellwether`) | **app** | the strip(s), the tray, and on first run the onboarding window |
| `Bellwether serve` (from the brain's login agent, or the app spawning a child) | **brain** | the headless listener on `127.0.0.1:47417`; never initialises Tauri, AppKit windows or WebKit |
| `bellwether <subcommand>` (a symlink on PATH; any known subcommand) | **CLI** | today's commands, same names, flags, output and exit codes |
| `bellwether` with no subcommand (lower-case symlink name) | CLI | prints usage, the same as `help` |

The CLI keeps every current command: `serve`, `record`, `status`, `token`,
`mute`/`unmute`, `show`/`hide`, `replay`, `shell …`, `autostart …`, `pets …`, `doctor`,
`hooks …`, `migrate`. Changes:

- `autostart install|uninstall|status` drives SMAppService instead of writing plists
  (D5).
- `shell dock-permission` becomes `permissions accessibility`, and there is a new
  `permissions automation <iTerm2|Terminal>`. Both ask the app to show the prompt, so
  macOS attributes the request to Bellwether and not to the terminal (D4, R7).
- `pets import <animations.xml>` is new (D8). Whether `pets fetch` stays is Q6.
- `migrate` gains the app-install source (D9).
- `shell --dev` disappears from the shipped binary; it is a repository-only workflow.

The CLI reaches PATH in one of two ways:

- the Homebrew cask's `binary` stanza symlinks
  `Bellwether.app/Contents/MacOS/Bellwether` to `bellwether` in Homebrew's bin [22];
- an optional onboarding step creates `~/.local/bin/bellwether` (no admin needed) and
  says so if that directory isn't on PATH.

The lower-case symlink name is also how the binary tells a CLI call from a Finder
launch. A second, lower-case executable inside the bundle is impossible: APFS is
case-insensitive by default, so `bellwether` and `Bellwether` would collide.

Files are written under `~/Library/Application Support/Bellwether/` and
`~/Library/Logs/Bellwether/` (see Configuration).

## Stack

| Piece | Choice | Why |
|---|---|---|
| Language | Rust 1.99 (installed), edition 2021 | one toolchain for brain, CLI and shell; the updater plugin needs ≥ 1.90 [4] |
| App framework | `tauri` 2.12.x (repo pins `=2.12.1`; 2.12.2 is current), `tauri-build` 2.7.1 | already the shell; it bundles, signs and notarizes [2][3] |
| Single instance | `tauri-plugin-single-instance` 2.5.2 (already used) | also carries argv from a second launch to the running app, which the CLI uses for actions that need the bundle's identity (D4) |
| Async runtime (brain) | `tokio` 1.53 (already in the lockfile through Tauri) | long-polls, timers and an actor-style state machine with no new crate; see D1 |
| HTTP server (brain) | `hyper` 1.12 + `hyper-util` 0.1.21 + `http-body-util` 0.1.5, HTTP/1 only (all already in the lockfile) | exact control of status, headers and the empty body; no framework layers (axum adds tower for five routes) |
| HTTP client (CLI, shell) | `ureq` 3.4.2 (already used) | blocking, small; `status --watch`, `mute`, `replay`, doctor probes |
| JSON | `serde` + `serde_json` 1.0.151 (already used) for config, snapshots and IPC; **a small hand-written lossless JSON module** for the hook installer only | serde_json's `arbitrary_precision` would keep number literals, but Cargo unifies features across the whole binary, so it would change number handling inside Tauri too (R2) |
| Regex | `regex` 1.13 (already in the lockfile) | redaction and user `bubble.redactPatterns`. It has no look-around or backreferences [25], so look-arounds are hand-translated (R1) |
| Unicode NFKC | `unicode-normalization` 0.1.25 (**new**, MIT/Apache) | `normaliseText` folds full-width letters before redaction |
| Hashing | `sha2` 0.10 (already used) | pet pins and the installer's changed-while-editing check; stay on 0.10 so there is one copy in the tree |
| Token randomness | `getrandom` 0.3 (already in the lockfile) | 24 random bytes, base64url, as today |
| macOS APIs | `objc2` 0.6.5, `objc2-foundation` / `objc2-app-kit` / `objc2-core-foundation` / `objc2-core-graphics` 0.3.2 (all in the lockfile), plus `objc2-service-management` 0.3.2 (**new**) | SMAppService [11]; optionally replacing hand-written FFI in `ax.rs`, `dock.rs`, `locator.rs` and `strips.rs`. Not required: the existing FFI works |
| Updates | `tauri-plugin-updater` 2.13.2 (**new**; brings reqwest 0.13 + rustls/ring) | signed updates from GitHub Releases [4] |
| File picker | `tauri-plugin-dialog` 2.8.1 (**new**), called from Rust only | the pet-import panel; the webview gets no dialog permission |
| CLI parsing | none (port of `parseArgs`) | the strict refusals (`--settings` without a value, unknown or `--__proto__` options, stray arguments) are a tested contract; a parser crate would change the messages |
| Dev only | Node 22.14 (installed) | runs the player tests and the conformance suite against the Rust binary; never shipped |
| Bundling | `@tauri-apps/cli` 2.12.1 (dev dependency) | `tauri build` for the `.app`, `.dmg` and updater artifacts |
| CI | GitHub Actions, macOS runners, `tauri-apps/tauri-action` v1.0.0 (pinned by commit SHA) [19] | builds, signs, notarizes, uploads assets and `latest.json` |

Rejected:

- `axum` (unneeded);
- `tiny_http` (last release 2022);
- `fancy-regex` (backtracking on text an attacker can shape through a prompt
  injection);
- `keyring` and `security-framework` (D3);
- `auto-launch` and `tauri-plugin-autostart`: both only write LaunchAgent plists or use
  AppleScript, with no SMAppService support [6][7];
- `externalBin` sidecars (D1).

Every new crate goes through `scripts/third-party.mjs --check`, so THIRD-PARTY.md stays
current. `ring` is "Apache-2.0 AND ISC", which needs a NOTICE line.

### Cargo layout

A workspace, so the platform-neutral parts can't pull in AppKit by accident:

- `crates/core`: config and defaults, the token file, the legacy `.env` reader, redact,
  normalise, flock, publisher, persist, the hook installer with its lossless JSON and
  its diff, the install record, the pet lock, status rendering, replay and record. It
  uses only std and serde.
- `crates/brain`: the HTTP server on tokio and hyper; depends on core.
- `app/` (moved from `shell/src-tauri/`): the `Bellwether` binary. It dispatches to the
  CLI, the brain or the Tauri app. All macOS code lives here, behind `cfg(target_os)`.

---

## Design decisions

### D1. The Rust brain: process model

**Decision: one binary, two processes.** The brain runs as its own process, started by
an SMAppService login agent (D5) whose `BundleProgram` is the app's own executable
with the argument `serve`. The strip is a second process from the same executable.

Why not in-process with the strip? The brain must keep answering hooks when the strip
is quit, hidden or has crashed. With hooks installed and nothing listening, every
prompt and tool call shows a "hook error" notice in the transcript [1] (and the README
records this as measured). A WebKit hang or an AppKit crash must never take `/hook`
down with it. The Quit item quits the strip only, as today.

Why not a separate helper binary (`externalBin`)? Tauri wants such binaries prebuilt
with a target-triple suffix [5], which means a second build step, a second signature
and a version that can drift from the app's. A brain and shell from different versions
would disagree about `/state`. One executable has one version and one signature, and
the updater swaps it in one go. The cost is that the brain process links
Tauri/WebKit/AppKit without using them. M1 measures that cost (R14). If it is bad, the
brain moves to a second executable built from the same workspace in the same `tauri
build`, and the decision stays "same bundle, same version".

**When the user hasn't enabled start at login**, the app spawns `Bellwether serve` as
a child. It keeps today's rules:

- the parent-pid watchdog is renamed `BELLWETHER_PARENT_PID`;
- the child exits within a second of the app dying, even on `kill -9`;
- a brain that was already running, or started by launchd, is never stopped by the app.

**Ports.** The brain listens on `127.0.0.1:47417`. That is a constant bind address,
with the port configurable as today, and it never binds anything but loopback.
Existing hook entries name this exact URL, which is what makes the migration
zero-downtime (D9).

**Concurrency: tokio, on a current-thread runtime in brain mode.**

- One task owns the flock and is the only writer. It works like an actor, with an
  mpsc channel in.
- Snapshots go out on a `tokio::sync::watch` channel carrying `rev`. That is exactly
  the `/state?after=<rev>&wait=` long poll.
- Timers (the flock tick, dwell, reap) live in the same task.
- `/hook` reads the body up to 8 MiB, answers before anything is parsed, then does a
  `try_send` to the actor. A full channel increments a new `dropped` counter in
  `/health` and never delays the answer.
- Threads would also work at this scale. tokio is chosen because it is already
  compiled in and hyper wants it.
- The shell keeps its current thread-plus-condvar design; it is not part of the port.

**`/hook` can never send a body, by construction.** The handler's return type has two
values, NoContent (204) and Forbidden (403, `Content-Length: 0`). Both convert to a
response whose body type is empty. Writing a body would not compile. This replaces the
JS source-scan test, which is kept for the JS brain until it is deleted. A black-box
test (every guard, every malformed input, body length 0) runs against both
implementations.

**What carries over unchanged.**

- The guards, in order: Origin → 403; Host not `127.0.0.1:<port>` or
  `localhost:<port>` → 403; Content-Type not `application/json` → 403; a token compared
  in constant time over sha256 digests → 403.
- OPTIONS → empty 405; anything else → empty 404; no CORS headers anywhere.
- Header timeout 10 s, request timeout 30 s.
- `/health` returns `{ok, name, version, pid, port, lastEventAt, mode, counters}` with
  no token required.
- `/state` and `/control` require the token.
- One release of legacy acceptance: the brain also accepts `X-Pasture-Token` and the
  `X-Pasture-*` terminal headers (the rename commit already does this in JS), and
  doctor warns.

**Shell-to-brain auth and the webview surface carry over as they are.** The shell (in
app mode) reads the token from the token file (D3) and sends `X-Bellwether-Token` on
`/state` and `/control`, exactly as today. The strip webview keeps its exactly six
commands and its single `strip` capability: `get_pet`, `get_snapshot`,
`report_hitboxes`, `sprite_clicked`, `shell_log`, `debug_snapshot` (which still refuses
unless started with `--snapshot`). The webview never talks to the brain and never sees
the token. `get_pet` changes only where pets come from (D8). The onboarding window gets
its own capability (D6). The private WebKit call (`_setWindowOcclusionDetectionEnabled:`,
guarded by `respondsToSelector:`) stays; see D4.

**Not painted into a Mac-only corner.**

- `crates/core` and `crates/brain` must `cargo check` for `x86_64-pc-windows-msvc` and
  `x86_64-unknown-linux-gnu` in CI from M2 on.
- Per-OS work sits behind narrow interfaces in `app/`: login items (SMAppService now;
  later the Task Scheduler or Run key, and a systemd user unit or XDG autostart),
  permissions, the locator, Dock or panel geometry, and app-data paths (macOS
  Application Support now; later `%APPDATA%` and the XDG dirs, through `dirs`, already
  in the tree).
- The Linux layer-shell strip is a later shell change, not a brain change.

### D2. Test parity for the port

The Node brain is the oracle until it is deleted. Parity is proven by running the same
tests against both implementations, not by reading code.

1. **One switch in `test/helpers.mjs`.** Every test that spawns `brain/cli.mjs` (about
   16 files import `child_process`, plus all the `tester-*` acceptance suites) goes
   through one helper. It reads `BELLWETHER_IMPL=node|rust`, and `BELLWETHER_BIN=<path>`
   for rust. Environment overrides (`BELLWETHER_HOME`, `BELLWETHER_CONFIG`,
   `BELLWETHER_HOOK_TOKEN`, a test HOME) are passed the same way to both. `npm test`
   runs node by default; `npm run test:rust` runs the conformance subset against the
   built binary.
2. **Shared vectors for pure functions.** Tests that import brain modules directly
   (redact, normalise, flock, hooks, doctor and so on) get their cases moved into JSON
   vector files, following the pattern `test/fixtures/env-vectors.json` already uses
   for JS and Rust together. Node tests and `cargo test` both read the same files:
   `redact-vectors.json`, `normalise-vectors.json`, `installer-vectors/` (settings
   bytes in, preview and bytes out), `diff-vectors.json`, `config-vectors.json`.
3. **The golden flock fixtures.** The 21 `s*.jsonl` and `*.expected.jsonl` pairs.
   `test/fixture-runner.mjs` drives the reducer with a fake clock (step 250 ms, tail
   3000 ms). The Rust side gets a matching cargo integration test that reads the same
   files and produces byte-identical transition lines. A build-time feature
   `conformance` (never on in release builds) also exposes hidden subcommands (`__fixture`,
   `__redact`, `__normalise`, `__json`, `__diff`). The JS runner can then compare both
   implementations from one place.
4. **Differential testing for the risky parts.** A dev-only script feeds the same
   inputs to the Node functions and the Rust `__` subcommands, and fails on any
   difference:
   - redaction: the vectors, plus generated strings mixing secret-like names,
     separators, quotes, URL credentials, base64 runs, full-width and zero-width
     characters, bidi marks and lone surrogates;
   - JSON round trips: generated settings files with odd layouts, big and exotic
     numbers (`1.0`, `1e2`, `-0`, `12345678901234567890`), escapes, duplicate and
     integer-like keys, `__proto__`;
   - `.env` lines (read-only now; D3).

   Live HTTP differential: post identical payload sequences to a Node brain and a Rust
   brain on two ports, and compare `/state` snapshots and `record` output line by line.
5. **Intentional divergences are written down, not hidden.** A short list in
   `test/DIVERGENCES.md`, each item with the test that pins the new behaviour. Expected
   entries:
   - JS reorders integer-like object keys (`"1"` before `"b"`) and Rust keeps file
     order (R2);
   - lone-surrogate escapes in hook payloads;
   - the `node` doctor check removed;
   - `token init` writes the token file, not `.env`;
   - the new `dropped` counter.

   Anything not on the list is a bug.
6. **What is ported to Rust unit tests and what stays JS.** Pure brain logic ends up as
   cargo tests over the shared vectors. The player tests (`player`, `layout`, `lambs`,
   `sprite`, `bubbles`, `horns`, `grass`, `labels`, `expr`, `stepper`) stay JS, because
   the player stays JS. The 49 existing cargo tests stay and move with the shell
   modules.
7. **How parity is shown per milestone.** Each port milestone (M3–M5) names the test
   files that must pass under `BELLWETHER_IMPL=rust`. CI publishes a parity table:
   test file, node result, rust result. A milestone is done when every file it names is
   green on both, the differential run (at least 10⁵ generated cases per area) shows
   zero differences, and DIVERGENCES.md has nothing unexplained.
8. **When the Node brain is deleted.** In M11, after:
   - the whole conformance suite has been green against Rust for one shipped release;
   - the migration has been run on at least one real repository install;
   - no Node-only test remains apart from the player and dev scripts.

   Deleting it means `brain/*.mjs`, `./bellwether` (the Node shim) and `bellwether.cmd`.
   The black-box suites stay, now running only against Rust, so the HTTP and CLI
   contracts stay pinned by tests written before the port.

**The three risky areas, called out:**

- **Redaction (R1).** JS regex differs from Rust's `regex` in ways the port must
  handle:
  - Look-behind and look-ahead are used in at least five patterns
    (`(?<![A-Za-z0-9_.-])`, `(?<=^|\s)`, `(?!-)`, `(?=@)`); they become a plain match
    plus an explicit check of the neighbouring characters.
  - `\d`, `\w`, `\s` and `\b` are ASCII in JS without the `u` flag, but Unicode in Rust
    by default; use explicit ASCII classes or `(?-u:…)`, and pin it with vectors
    containing Arabic-Indic digits and NBSP.
  - JS replaces left to right with the `g` flag; the scan-and-replace helpers' order
    must match.
  - NFKC tables can differ by Unicode version between Node's ICU and the crate. Pin it
    with the full-width and compatibility vectors, and accept the differential's
    verdict.
  - Truncation is by code point (`[...s]`), which matches Rust `chars()`. Keep it that
    way.
- **User `bubble.redactPatterns`.** They are JS-flavoured today, with `(?i)`
  supported. Under `regex`, look-around and backreferences become a config error
  (exit 3) naming the pattern, and doctor says why. That is a stated divergence.
  `regex` guarantees linear time, which is a safety gain.
- **Hook-installer byte fidelity (R2).** "Install then uninstall gives back the same
  bytes" and the `layoutKept` preview depend on reproducing Node's
  `JSON.parse`/`JSON.stringify` closely. The lossless module must:
  - keep every number literal verbatim;
  - keep key order, and on a duplicate key keep the first position with the last value
    (what JS does);
  - treat `__proto__` as an ordinary key;
  - escape strings exactly as `JSON.stringify` does: `\"`, `\\`, `\b \f \n \r \t`,
    other controls as lower-case `\u00xx`, lone surrogates as `\udxxx`, `/` and
    non-ASCII written literally;
  - detect indent and EOL the way `serialise()` does.

  The diff (`diff.mjs`, an LCS) is ported line for line, because the preview text is a
  contract.
- **`.env` parsing (R3).** Risk drops sharply because the app no longer writes `.env`
  files (D3). The Rust reader is used only to import a legacy token. It reuses
  `settings.rs`'s `parse_env_value`, already checked against `env-vectors.json`. The
  in-place `.env` writer (`token init` with fill-in-place, CRLF, symlinks, `.bak`) is
  retired. Its tests move to DIVERGENCES.md as "retired with the Node CLI".

### D3. Token and config storage

**Decision: a 0600 file, not the Keychain.**

- Path: `~/Library/Application Support/Bellwether/hook-token`, inside a 0700
  directory, holding one line: the token.
- Format check as today: 16 or more of `[A-Za-z0-9_-]`, and known example values are
  refused.
- `BELLWETHER_HOOK_TOKEN` in the environment overrides it (tests, CI).
- `BELLWETHER_HOME` overrides the whole directory (tests).

Why not the Keychain:

- The token's job is anti-spoofing: it stops another local app or a browser tab from
  posting fake bubbles. It has to sit as a **literal header value** in
  `~/.claude/settings.json`, and in the installer's backups, because Claude Code sends
  it with every hook call.
- Claude Code can interpolate `$VAR` into headers only from its own environment,
  limited by `allowedEnvVars` and any `httpHookAllowedEnvVars` policy [1]. Sessions
  started from arbitrary terminals won't have a GUI app's secret in their environment.
- So the plaintext copy in settings.json is unavoidable, and the Keychain would add
  approval prompts with no confidentiality gain. It would also add Keychain ACLs tied
  to the code signature (prompts after unsigned dev builds), possible access failures
  from a launchd-started process, and a second store to keep in sync on Windows and
  Linux.

**How hooks carry it:** unchanged. `hooks install` writes
`"X-Bellwether-Token": "<token>"` literally, plus the `$TERM_PROGRAM`-style terminal
headers through `allowedEnvVars`. It never sets `onFailure` (default `"continue"`). Any
Bellwether entry with `onFailure: "block"` is a doctor **FAIL**: with the brain down,
it would block every tool call [1].

**Rotation:** `bellwether token rotate [--write]`:

1. write the new token file;
2. preview and rewrite the hook entries (same exact-URL ownership);
3. signal the brain to reload, through `/control` with the old token.

Running Claude Code sessions pick up settings edits through their file watcher [1].
For one minute after a rotation the brain accepts both tokens, so in-flight sessions
don't show errors.

**Layout** under `~/Library/Application Support/Bellwether/`:

| Item | What it is |
|---|---|
| `hook-token` | 0600 |
| `config.json` | optional; compiled-in defaults otherwise; `config.example.json` is shipped in Resources and `doctor` shows the path |
| `state.json` | muted, plus per identity its colour, slots and label; same shape and same pretty-printing as today |
| `shell-state.json` | hidden, show labels |
| `installs.json` | which event keys an install created, keyed by the settings file's realpath |
| `backups/` | settings backups, 0600, timestamped |
| `pets/<name>/animations.xml` | imported pets, plus `pets.user.lock.json` with their sha256 |
| `recordings/` | `record` output |
| `legacy/` | plists and files moved aside by a migration, kept for rollback |

Logs go to `~/Library/Logs/Bellwether/brain.log` and `shell.log`.

### D4. Bundle, signing, notarization, identity

**Bundle id.** `local.bellwether` (set only in `tauri.conf.json`) is fine for local
builds but must not ship. It must become a reverse-DNS id the maintainer controls
before the **first signed build anyone installs**, and never change after that. macOS
ties TCC grants (Accessibility, Automation) to the code's designated requirement,
which for Developer ID is the bundle id plus the Team ID. Keeping both stable is what
lets grants survive updates; changing either drops them [17][18]. The ad-hoc builds
lose them on every rebuild for the same reason. Placeholder in this plan:
`<reverse-dns>.bellwether` (Q1).

Labels derived from it:

- the brain agent plist: `<reverse-dns>.bellwether.brain.plist`;
- the single-instance id.

`local.bellwether` stays the identifier of repository dev builds, so a dev build and
the installed app never share TCC entries or login items. Doctor shows which identity
it runs as.

**Build products** (`tauri build`):

- `Bellwether.app`;
- `Bellwether_<ver>_universal.dmg`;
- the updater pair `Bellwether.app.tar.gz` plus `.sig` (`createUpdaterArtifacts:
  true`) [4];
- target `universal-apple-darwin` (**ASSUMPTION**, Q3);
- `minimumSystemVersion` stays `13.0`, which SMAppService requires [8];
- `mainBinaryName: "Bellwether"`;
- the original pet in `bundle.resources`.

**Hardened runtime:** on. Tauri's `bundle.macOS.hardenedRuntime` defaults to true
[28]. Notarization requires it, plus a Developer ID Application certificate, a secure
timestamp, and no `get-task-allow` [12].

**Entitlements:** only what is used.

- `com.apple.security.automation.apple-events = true`, with
  `NSAppleEventsUsageDescription` in a merged `Info.plist` [3][16]. The locator sends
  Apple Events to iTerm2 and Terminal through `/usr/bin/osascript`. Because the app is
  the responsible process, it needs the entitlement and the usage string.
  **ASSUMPTION**, verified in M1: the prompt appears naming Bellwether and `-1743` does
  not occur. If it does, the locator moves to in-process `NSAppleScript`.
- Nothing else is expected:
  - no JIT or unsigned-memory exceptions, because WKWebView runs JavaScript in WebKit's
    own XPC processes;
  - no sandbox;
  - no network entitlements, because the app is unsandboxed and binds loopback only;
  - Accessibility needs no entitlement, only the TCC grant [18].

  M1 proves this by running the hardened, signed build through every feature.
- `LSUIElement` stays effectively true (the app already uses the Accessory activation
  policy). The onboarding window activates the app explicitly.

**Notarization:** `notarytool` through the Tauri bundler's environment variables [2].

- App Store Connect API key: `APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_PATH`.
  Recommended over the Apple ID route (`APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`)
  because the key can be revoked alone and has no 2FA involved (Q8).
- The bundler notarizes and staples the `.app`.
- CI then notarizes the **DMG** too and staples it (`notarytool submit --wait`, then
  `stapler staple`). Disk images are a supported container [12][14], and a stapled DMG
  opens cleanly offline.
- Checks: `codesign -vvv --deep --strict`, and `spctl -vvv --assess --type exec` on the
  app; `spctl --assess --type open --context context:primary-signature` on the DMG [13].

**The private WebKit selector.** Notarization is "not App Review": it is an automated
scan for malicious content and signing problems [12], so a guarded SPI call is not a
reason for rejection. It would be for the App Store, which is out of scope. Its risk is
technical: a future macOS could remove the selector (R5). The guard already makes that
fail soft (the strip may get throttled). Two additions:

- the shell logs whether it was applied;
- doctor shows a WARN if not.

**CLI-initiated prompts.** TCC attributes a request to the responsible process. A
prompt raised by the CLI under Terminal would ask for **Terminal's** access (R7). So
`bellwether permissions …` and `autostart …` don't call the APIs themselves. They hand
the action to the app:

- through LaunchServices: `open -b <bundle-id> --args --action <name>`;
- if the app is running, the single-instance plugin forwards it;
- the app does the action, writes the outcome to
  `Application Support/Bellwether/actions/<name>.json`, and the CLI waits up to 30 s
  for that file;
- if SMAppService calls turn out to work directly from the CLI process (M1 checks),
  the autostart commands skip the hand-off.

### D5. Login items

**Decision:** SMAppService for both processes. No plists are written to
`~/Library/LaunchAgents` any more. Both items show up under Bellwether in System
Settings > General > Login Items [8][10].

- **The brain:** `SMAppService.agent(plistName:)` with
  `Contents/Library/LaunchAgents/<reverse-dns>.bellwether.brain.plist` inside the
  signed bundle, the required location [9]. The bundler's `bundle.macOS.files` puts it
  there [3]. Contents:
  - `Label`;
  - `BundleProgram`: `Contents/MacOS/Bellwether`;
  - `ProgramArguments`: `Bellwether serve`;
  - `KeepAlive {SuccessfulExit: false}` (restart on crash);
  - `ThrottleInterval 10`;
  - `ProcessType Background`;
  - `LimitLoadToSessionType Aqua`.

  The same keys as today's `local.bellwether.brain`, minus the paths: no node path, no
  working directory, and logs go to `~/Library/Logs/Bellwether`.
- **The strip:** `SMAppService.mainApp`. This keeps today's behaviour: it starts at
  login, and Quit keeps it quit until the next login. The `--no-spawn-brain` argument
  goes away; the app checks `/health` and spawns a child only when no brain answers.
- **Status:** `autostart status` and onboarding report `notRegistered`, `enabled`,
  `requiresApproval` or `notFound`. On `requiresApproval` they offer
  `openSystemSettingsLoginItems` [11].
- **After an update:** a running brain is still the old executable. When the app
  starts it compares its version with `/health`'s `version`. If they differ, it asks
  launchd to restart the agent (`launchctl kickstart -k gui/$(id -u)/<label>`) and
  waits for `/health` to report the new version. The gap must be under 1 s (measured
  in M9; R4).
- **Legacy agents.** These are removed by the migration (D9), never silently at
  startup:
  - `local.bellwether.brain` and `local.bellwether.shell` (written by the renamed repo
    version);
  - `local.pasture.brain` and `local.pasture.shell` (written before the rename).

  On every start the app checks for them and shows one line in the tray and in doctor
  if any are still loaded: two brains would fight over port 47417.

### D6. First-run onboarding

A small, normal window (label `onboarding`, about 520×600, not the strip). It opens on
first launch, from the tray item "Set up Bellwether…", and from `open -b <id> --args
--onboarding`.

Steps, in this order. The order is what keeps a brain always running whenever hooks
are installed:

1. **Earlier install found?** Shown only when the migration scan (D9) finds something.
   Shows the migration preview and runs it on Confirm. Replaces steps 2–3 for those
   users.
2. **Start at login.** Registers the brain agent and the main app. Recommended on.
   **ASSUMPTION** (Q12): if it stays off, step 3 shows a warning that hooks will print
   errors whenever the app isn't running.
3. **Hooks.** Shows the exact diff `hooks install` would apply to
   `~/.claude/settings.json`, with the token masked (first four characters and "…"), the
   number of other tools' entries left untouched, and any layout warning. **Install**
   applies exactly that preview. It is refused if the file changed since the preview
   (same sha256 check as the CLI), and the user is shown a fresh preview. The
   exact-URL ownership rule, the install record and the backup all work as in the CLI.
   An **Uninstall** button uses the same preview-then-confirm.
4. **Optional: sheep on the Dock** (Accessibility). Explains what it's for, then
   triggers `AXIsProcessTrustedWithOptions` with the prompt and offers the Settings
   pane. Shows the live status.
5. **Optional: click a sheep to jump to its tab** (Automation, per app: iTerm2,
   Terminal). Sends one harmless Apple Event (asks iTerm2 for its version) to raise the
   prompt. Shows the status, and says that refusing still brings the app forward.
6. **Pet.** Bellwether's own sheep (default), or a previously imported pet, or
   **Import…** (Rust-side file panel, D8). Shows a still preview of the pet's first
   frame.
7. **Optional: command-line tool.** Creates `~/.local/bin/bellwether` (skipped when a
   Homebrew symlink exists).
8. **Done.** Ends with `doctor` results in plain words.

**The new webview surface: the minimum, in its own capability.**
`capabilities/onboarding.json` applies to `windows: ["onboarding"]` and allows only the
commands below. The strip capability is unchanged (still the six). No plugin
permissions are granted to either webview: no fs, shell, http, opener, dialog,
updater, clipboard. The CSP stays `default-src 'self'` with no remote content. Each
command also checks `window.label() == "onboarding"` in Rust, as a second wall.
Arguments are enums or opaque ids, never paths, file contents or tokens.

| Command | Arguments | Returns / effect |
|---|---|---|
| `setup_status` | none | hooks state, login-item states, AX trusted, Automation per app, pets list, migration found, CLI-tool state |
| `hooks_preview` | `action: "install" \| "uninstall"` | `{planId, diffText (token masked), layoutKept, warnings, othersUntouched}`; the plan is kept on the Rust side with the file's sha256 |
| `hooks_apply` | `planId` | applies that stored plan only; refuses if the file's sha256 changed |
| `login_items_set` | `enabled: bool` | registers or unregisters both services; returns the new statuses |
| `permission_request` | `kind: "accessibility" \| "automation-iterm2" \| "automation-terminal"` | raises the system prompt or opens the pane |
| `pet_select` | `name` (must be a bundled or imported pet name) | writes `pets.default` in config.json; the strip reloads its pet |
| `pet_import` | none | opens the native panel on the Rust side, validates, copies, pins; returns the new name or an error |
| `migration_preview` / `migration_apply` | none / `planId` | D9 |
| `cli_tool_install` | none | creates the symlink; returns the path and whether it's on PATH |
| `setup_close` | none | marks onboarding done in shell-state.json and closes the window |

That is 11 commands, each with an autogenerated permission file like the existing
ones. A test asserts each capability's exact permission list, so adding a command
without a review fails CI.

### D7. Updates and release

- **Updater:** `tauri-plugin-updater`.
  - Endpoint:
    `https://github.com/<owner>/bellwether/releases/latest/download/latest.json`.
  - `pubkey` is in `tauri.conf.json`. The private key comes from `tauri signer
    generate` [4].
  - Checks run from Rust only (`UpdaterExt`) [4]: at start and every 24 h, and from
    the tray's "Check for Updates…".
  - The prompt is a native dialog. Install happens on confirm, then a relaunch, then
    the brain-restart step in D5.
  - No updater permission is given to any webview.
  - Behaviour on a found update is Q7.
- **GitHub Releases** hold the DMG, the `.app.tar.gz` and `.sig`, `latest.json` and
  `SHA256SUMS`.
  - `latest.json`'s `platforms` keys are `darwin-aarch64` and `darwin-x86_64` [4], both
    pointing at the universal tarball (verify that tauri-action emits both for a
    universal build [19]; otherwise CI writes them).
- **Homebrew cask.**
  - Start in the project's own tap, `<owner>/homebrew-tap`. Homebrew's official cask
    repo has notability rules [23], and casks that fail Gatekeeper lose support there
    from 2026-09-01 [24]. A notarized app meets that, so the official repo stays open
    for later (Q5).
  - Stanzas [22]:
    - `app "Bellwether.app"`;
    - `binary "#{appdir}/Bellwether.app/Contents/MacOS/Bellwether", target: "bellwether"`;
    - `auto_updates true` (the app updates itself);
    - `uninstall quit: "<bundle-id>"`;
    - a `zap` of the Application Support, Logs and Preferences paths.
  - The cask can't reach into `~/.claude/settings.json`. Its caveats tell the user to
    run `bellwether hooks uninstall --write` before `brew uninstall`. Doctor also notices
    hooks pointing at a brain that no longer exists.
  - CI opens a PR on the tap with the new version and sha256.
- **Release CI** (`.github/workflows/release.yml`), with secrets handled carefully
  [20][21]:
  - Runs only on a pushed `v*` tag. Never on `pull_request`; `pull_request_target` is
    never used.
  - The job uses a GitHub **environment** `release` with a required reviewer (the
    maintainer) and the tag pattern as a deployment rule. All signing secrets live in
    that environment, not at repository level.
  - Secrets:
    - `APPLE_CERTIFICATE` (base64 `.p12`, Developer ID Application only) and
      `APPLE_CERTIFICATE_PASSWORD`;
    - `APPLE_SIGNING_IDENTITY`;
    - `APPLE_API_ISSUER` and `APPLE_API_KEY`, plus the `.p8` stored base64 and written
      to a temp file for `APPLE_API_KEY_PATH`;
    - `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`;
    - a fine-grained token for the tap PR only.
  - The certificate is imported into a throwaway keychain created for the job, with a
    random password, and deleted in an `always()` step. Temp key files are deleted the
    same way.
  - `permissions:` default `contents: read`. Only the upload step's job gets
    `contents: write`.
  - Every third-party action is pinned by full commit SHA.
  - No secret goes into a log. `set -x` is never used in signing steps.
  - The updater private key is also kept offline by the maintainer. Losing it means
    existing installs can never be updated (R9).
  - A separate `ci.yml` on every PR, with no secrets: `npm test`, `npm run test:rust`
    (conformance), `cargo test --workspace`, cross-target `cargo check` of core and
    brain, the THIRD-PARTY check, and an **unsigned** `tauri build`.

### D8. Original sheep art

**Recommendation: commission it from a pixel artist, as a written work-for-hire or
with a copyright assignment to the maintainer, delivered as layered source files plus
per-frame PNGs.** The maintainer licenses it **CC-BY-4.0** (Q4).

- Drawing it in-house is the fallback if the budget isn't there. It takes longer but
  has the same clean title.
- Generating it and then hand-cleaning it is not recommended. The US Copyright Office's
  January 2025 report says prompts alone don't make the user the author [26]. A sheet
  that is mostly generated may not be protectable or cleanly licensable, and its
  provenance is hard to state in NOTICE. AI used as an assistive tool by the artist
  (for example to suggest in-betweens they then redraw) is fine and gets disclosed.

**Specification handed to the artist** (`pets/bellwether/SPEC.md`, written in M10):

- Animations for every key Bellwether uses:
  - walk, run, graze (eat), sleep (enter plus loop), alert (front-facing bounce),
    turn, trip, sit, dizzy, chew, arrive, leave, hop, lie;
  - a front-facing pose used by alert, and the roll used when an alert is confirmed.
- Side frames face the direction the desktopPet format expects, as esheep64 does
  (verified against the player's mirroring code).
- Frame size the same as esheep64's tile, so `slotWidthPx`, `scale: 1.5` and the layout
  constants hold. Drawn so the sheep is crisp at the default scale on 2× displays.
- **Horn mask:** horns drawn only in the three-tone magenta family the tinter
  recognises: hue 285–315°, saturation ≥ 0.35, mid tone `#b300b3` (the session colour
  lands there exactly), dark `#730073`, highlight `#ff7aff`. No other pixel may fall in
  that hue window. `#FF00FF` is reserved for the transparency key; a PNG with alpha and
  `transparency` unset is preferred.
- **Lambs** use the same sheet at `lambScale` 0.6. The art must stay readable at that
  scale, which is a review gate. Separate lamb frames would be a later change (Q4
  sub-question).
- Light and dark wallpapers: the outline must read on both.

**Pipeline** (deterministic, in the repository):

1. The artist delivers `pets/bellwether/src/` (layered source) and
   `pets/bellwether/frames/<anim>_<nn>.png`.
2. A declarative `pets/bellwether/pet.json` lists each animation's frames, intervals,
   movement and `next` transitions. It is code, MIT.
3. `scripts/build-pet` packs the frames into one sheet, writes a desktopPet-format
   `animations.xml` with the PNG embedded as base64, and records its sha256 in
   `pets.lock.json`. Since the names are Bellwether's own, `animationMap` for this pet
   is the identity.
4. The result is committed and bundled as a resource. `get_pet` loads bundled pets by
   the pin compiled into the binary, and imported pets by
   `pets.user.lock.json`.

**"Load any desktopPet pet" stays.**

- `bellwether pets import <file>` and onboarding's Import… validate the file:
  - at most 16 MiB;
  - UTF-8;
  - no DOCTYPE or ENTITY;
  - it parses;
  - tiles at most 64×64.
- They copy it to `Application Support/Bellwether/pets/<name>/`, pin its sha256 in
  `pets.user.lock.json`, and propose an `animationMap` by name matching. A file changed
  after import is refused, as today.
- A migrating user's fetched `esheep64` is imported this way: it's their local copy,
  and nothing is redistributed.

**Acceptance** (automated, then human):

- (a) `Bellwether --selftest` parses the pet with the webview's real parser;
- (b) every `animationMap` key resolves, and every frame index is below
  tilesx × tilesy;
- (c) a horn test like `test/horns.test.mjs`, now run on the real sheet: every pixel
  either is horn or sits outside the hue window;
- (d) for each of the 8 palette colours, the tinted horn differs from the wool and
  outline by a minimum ΔE;
- (e) the sha256 pin matches;
- (f) `--snapshot` grids of every behaviour at sheep and lamb scale on light and dark
  backgrounds, reviewed and signed off by the maintainer;
- (g) provenance: the signed agreement on file (not in the repo), the artist's credit
  and the CC-BY-4.0 text in NOTICE, and the source files committed.

NOTICE keeps the desktopPet (MIT) credit for the format, and the statement that the
eSheep art isn't included.

### D9. Migration for existing users

Two kinds of existing install:

- the **renamed repository version**: `local.bellwether.*` LaunchAgents, `.env` with
  `BELLWETHER_HOOK_TOKEN`, hooks with `X-Bellwether-Token`, the repo's `data/`;
- the **pre-rename version**: `local.pasture.*`, `PASTURE_HOOK_TOKEN`, `X-Pasture-*`.
  The committed `bellwether migrate` already handles this one inside a repository.

The app's migration does both in **one guided step**, as onboarding step 1 or
`bellwether migrate --to-app [--from <repo>] [--write]`. It reuses the committed
command's proven sequence (back up, boot out, load new, verify, restore on failure).

**Finding the old install**, in this order:

1. `--from`;
2. the `WorkingDirectory` of `~/Library/LaunchAgents/local.bellwether.brain.plist`, or
   failing that `local.pasture.brain.plist`;
3. asking (onboarding offers a folder panel; the CLI says to pass `--from`).

Hooks alone, with no agents, are enough to start: the token is then read from those
entries.

**Preview** (nothing changes without Confirm or `--write`):

- the files to copy;
- the agents to stop and move aside;
- the hook diff;
- what stays behind (the repository itself is never modified or deleted).

**Order on Confirm:**

1. **Token.** Use the value from the old `.env`, or failing that the value in the
   existing hook entries. Write it to `hook-token`. Keeping the same value means
   running sessions keep working.
2. **Data.** Copy `config.json` (obsolete keys such as `brain.nodePath` dropped, shown
   in the preview), `data/state.json` (colours, slots, mute), `data/shell-state.json`
   and `data/installs.json` (realpath keys unchanged), and `data/backups/` (0600) into
   Application Support. Copy fetched pets as imported pets. Each item is copied only if
   the target doesn't exist, unless `--force`.
3. **Port check.** Use the existing `porthold` logic. If 47417 is held by something
   other than the old brain, stop and say what holds it.
4. **Swap brains:**
   - `launchctl bootout` the old brain agent and the old shell agent, by label;
   - stop the legacy shell by its exact pid from `data/shell.pid` if it was started
     without launchd (never by name);
   - move the old plists to `legacy/`;
   - register the SMAppService agent and mainApp;
   - wait for `/health` to show the new brain and for `/state` to accept the token.

   If that fails within 10 s: unregister, move the plists back, bootstrap them, and
   report. The old setup is left running.

   The gap with no brain is under 1 s. Hooks fired in that gap print one hook-error
   notice; the preview says so.
5. **Hooks.** Rewrite Bellwether's entries (exact URL `http://127.0.0.1:47417/hook`)
   to the current header names. Today's ownership and backup rules apply. Pre-rename
   `X-Pasture-*` entries count as Bellwether's through the committed
   `isLegacyHandler` rule.
6. **Accessibility leftovers.** The ad-hoc `bellwether-shell` (or `pasture-shell`)
   entry may remain in Accessibility. Say so, and offer the pane. Bellwether doesn't
   reset TCC for another identity.
7. **Doctor.** Run it, and show the result.

**Idempotent:** a second run says "nothing to migrate". A run that stopped part-way
finishes what is left, as the current `migrate` does.

**Rollback:** `bellwether migrate --undo [--write]`. It unregisters the login items,
moves the plists back from `legacy/` and bootstraps them, and restores the settings
file from the backup taken in step 5 (after the same sha256 check). It leaves
Application Support in place.

---

## Auth and permissions

- **Hook auth:** the token header on `/hook`, `/state` and `/control`, compared in
  constant time; loopback bind only; the Origin, Host and Content-Type guards (D1).
  Bellwether's hooks can never make a decision for the user, because `/hook` can't
  send a body (D1, enforced by the type) and its entries never set `onFailure` (D3).
- **What Bellwether may write**, enforced in code:
  - in `~/.claude/settings.json`, only matcher groups and handlers whose URL is exactly
    Bellwether's, with `assertOthersPreserved` checked before every write. It never
    writes the project, local or managed settings files. The `--out` guard refusing
    Claude Code settings files is ported as is;
  - launchd state only through SMAppService for its own two services, and `launchctl
    bootout` only for the four legacy labels, after checking the plist's contents
    point at a Bellwether or pasture brain or shell;
  - it never kills a process by name; only a pid it started or read from its own pid
    file, after checking that pid's executable path.
- **macOS permissions** (all optional; the strip works with none):
  - Accessibility, for the Dock (no entitlement);
  - Automation for iTerm2 and Terminal, for click-to-focus (entitlement plus usage
    string);
  - Screen Recording is never needed.
- **Network:** loopback listener only. Outbound HTTPS only for the update check (and
  `pets fetch` if kept, Q6).

**What the maintainer does by hand, once:**

1. In the Apple Developer account, create a **Developer ID Application** certificate.
   Export it with its private key as `.p12`, with a strong password.
2. In App Store Connect > Users and Access > Integrations, create an **API key** for
   notarization with the lowest role that allows notary submissions. Note the Issuer
   ID and Key ID, and download the `.p8` (it can be downloaded only once).
3. Choose the permanent bundle id (Q1).
4. Run `npx tauri signer generate -w <a path outside the repo>` and back up the
   private key and its password offline. Put the public key in `tauri.conf.json`.
5. In the GitHub repository, create the environment `release`. Require the
   maintainer's review, limit it to `v*` tags, and add the secrets listed in D7.
6. Create the tap repository `homebrew-tap` and a fine-grained token that can only
   open PRs there.
7. For the art: brief the artist with SPEC.md, and sign the assignment or work-for-hire
   agreement before work starts.

## Configuration

**Environment variables** (no hub `.env`; the app reads none of these in normal use):

| Variable | Purpose | Example |
|---|---|---|
| `BELLWETHER_HOOK_TOKEN` | overrides the token file (tests, CI) | `testTokenNotReal_abcdefghijklmnop` |
| `BELLWETHER_HOME` | overrides the Application Support directory (tests) | `/tmp/bw-test-1` |
| `BELLWETHER_CONFIG` | read-only alternative config file (exists today) | `~/bw-test.json` |
| `BELLWETHER_SHELL_TRACE` | shell trace logging (exists today) | `1` |
| `BELLWETHER_PARENT_PID` | marks a brain started by the app (internal) | set by the app |
| `BELLWETHER_IMPL`, `BELLWETHER_BIN` | test harness only: which brain to test | `rust`, `app/target/release/Bellwether` |
| CI only | `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_API_ISSUER`, `APPLE_API_KEY`, `APPLE_API_KEY_PATH`, `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` [2][4] | (secrets) |

Legacy `PASTURE_*` names keep the one-release adoption the rename commit already has
in the Node CLI. The Rust CLI honours them only during migration.

**`config.json`** (in Application Support; every key optional, same schema as today's
`config.example.json`):

| Key | Change | Example |
|---|---|---|
| `port` | unchanged; `host` still rejected | `47417` |
| `brain.nodePath` | **removed**; doctor WARNs "obsolete, ignored" | none |
| `pets.default` | default becomes `bellwether` | `"bellwether"` |
| `pets.animationMap.<pet>` | unchanged; not needed for the bundled pet | `{"walk":"walk",…}` |
| `pets.palette`, `lambScale`, `tint`, `scale` | unchanged | `["#FF8C00", …]` |
| `behaviour.*`, `bubble.*` | unchanged; `redactPatterns` now validated by Rust `regex` (look-around rejected) | `"(?i)(token\|secret)=\\S+"` |
| `strip.*`, `privacy.*`, `fade.*`, `focus.*`, `locator.*` | unchanged | as today |
| `autostart` | **removed** (login-item state lives in SMAppService) | none |
| `updates.check` | **new**: `"prompt"` (default) or `"off"` | `"prompt"` |

The bundle carries `config.example.json` in Resources; `doctor` prints its path.

## Data model

- **Hook input:** Claude Code's JSON, unchanged. 16 subscribed events (SessionStart and
  Setup can't take HTTP hooks [1]). Terminal headers come through `allowedEnvVars`.
- **Hook entry written** (per event, one matcher group):
  `{"type":"http","url":"http://127.0.0.1:<port>/hook","timeout":1,"headers":{"X-Bellwether-Token":"<token>","X-Bellwether-Term-Program":"$TERM_PROGRAM",…},"allowedEnvVars":[…]}`.
  It has no `onFailure`. `timeout` is in seconds [1].
- **`/health`, `/state` Snapshot, `/control` messages** (`mute`, `unmute`, `dismiss`,
  `placed`, plus the new `reload-token`): the current JS shapes are the contract, pinned
  by the conformance suite. `/health` adds `counters.dropped`.
- **Persisted files:** `state.json`, `shell-state.json`, `installs.json`, and the pet
  locks, in today's shapes, written with 2-space pretty JSON plus a newline, 0600,
  through a temp file and rename. `hook-token` is one line, 0600.
- **Action hand-off** (D4): `actions/<name>.json` with
  `{action, startedAt, finishedAt, ok, message}`. Written by the app; read and then
  deleted by the CLI.
- **Updater:** `latest.json` with `version`, `notes`, `pub_date`, and
  `platforms.{darwin-aarch64,darwin-x86_64}.{url,signature}` [4].
- **Brain login agent plist** (in the bundle): see D5.

---

## Milestones

The order puts the riskiest unknowns first:

1. Can one signed, notarized, hardened executable serve as an SMAppService brain and
   the strip, with the Automation and WebKit behaviour intact?
2. Can the port match the Node brain's byte-level contracts?

The art (M10) has a long lead time and no code dependency. **Brief the artist during
M1.**

### M1. Signed skeleton: one binary, three modes, SMAppService brain

Builds:

- the Cargo workspace with `app/` (the current shell moved, still talking to the Node
  brain for the snapshot);
- argv dispatch (app, `serve`, CLI);
- a minimal Rust `serve` with `/health` and `/hook` (all four guards, the 204/403 type,
  timeouts, legacy header acceptance);
- the bundled agent plist;
- SMAppService registration from the app;
- the permanent bundle id in `tauri.conf.json`;
- entitlements and the merged Info.plist;
- a local `tauri build` signed with the Developer ID certificate and notarized with the
  API key.

**Done when:**

- `codesign -vvv --deep --strict` and `spctl --assess --type exec` pass on the `.app`;
  `notarytool` reports Accepted; `stapler validate` passes;
- `codesign -d --entitlements -` shows only `automation.apple-events`;
- with the login item on, after logging out and in, `curl` to `/health` shows the Rust
  brain;
- quitting the strip from the tray leaves `/hook` answering 204 (checked with a
  correctly tokened POST);
- System Settings > Login Items lists Bellwether;
- the hardened, signed strip shows sheep (against the Node brain's `/state` on another
  port, or `--demo`) and the trace log says the WebKit call was applied;
- a sheep click in iTerm2 raises an Automation prompt naming Bellwether, and the tab
  is selected;
- an Accessibility grant to the signed app survives a rebuilt, re-signed build with the
  same id and team;
- the hook-route tests (`hook-route.test.mjs` and the guard cases from
  `brain-server.test.mjs`) pass under `BELLWETHER_IMPL=rust`;
- recorded in the PR: the brain process's RSS at idle and its cold start time, and
  whether SMAppService calls work from the CLI under Terminal.

#### M1 status (developer, 2026-10-12, by-hand checks 2026-10-13): done

Built:
- the brain crate, `crates/brain`: `/hook` (four guards, the `HookAnswer` type,
  timeouts) and `/health`;
- argv dispatch, `shell/src-tauri/src/dispatch.rs`;
- the M1 CLI, `cli.rs`: `serve`, `doctor`, `autostart`, `version`, `help`;
- the app-side action hand-off, `actions.rs`;
- SMAppService, `login_items.rs`;
- the bundled agent plist, the entitlements, the merged Info.plist and the
  distribution overlay with the permanent id: `shell/src-tauri/macos/` and
  `tauri.dist.conf.json`;
- `scripts/build-app.mjs`;
- the black-box suite `test/hook-route-blackbox.test.mjs`, run by `npm test` against
  Node and by `npm run test:rust` against Rust.

Progress against **Done when**. "The signed build" is the one the maintainer made
with `node scripts/build-app.mjs --notarize --test-port 47418 --test-home /tmp/bw-m1`
(Oct 12, 22:03).

- [x] codesign, spctl, notarytool and stapler pass on the Developer ID build.
  - The first attempt stopped at a keychain dialog for `codesign`, until the
    maintainer chose Always Allow.
  - On the signed build, `codesign --verify --deep --strict -vvv` says "valid on disk
    / satisfies its Designated Requirement". `codesign -dvv` shows
    `flags=0x10000(runtime)`, `Authority=Developer ID Application: Fanzoo Technology,
    Inc. (TEAM ID)`, a secure `Timestamp=Oct 12, 2026 at 10:03:16 PM` and `Notarization
    Ticket=stapled`.
  - `xcrun stapler validate` says "The validate action worked!".
  - `spctl -a -vvv -t exec` says "accepted, source=Notarized Developer ID".
- [x] `codesign -d --entitlements -` shows only
  `com.apple.security.automation.apple-events`.
- [x] Logging out and in brings the Rust brain back. By hand, step 1: after login the
  agent ran as `Bellwether serve --port 47418` with ppid 1. Registering also starts it at
  once.
- [x] Quitting the strip leaves `/hook` answering: a correctly tokened POST got 204 with
  0 bytes. Tested on the ad-hoc build, with a `--demo` strip that quit through
  `--quit-after`, not the tray's Quit.
- [x] System Settings > Login Items lists Bellwether. By hand, step 2.
- [x] The hardened, signed strip shows sheep, and the trace says the WebKit call was
  applied. On the signed build, `--selftest` printed `selftest: done PASS`. A `--demo`
  launched through LaunchServices logged "webview occlusion detection disabled: the
  page stays visible while the strip is on screen" and "heartbeat 3, 13 sprites, page
  visible", and its snapshot shows the flock.
- [x] An Automation prompt naming Bellwether appears for iTerm2. By hand, step 3; F2
  shows the attribution. Selecting the tab after a sheep click was not reported
  separately: it needs a flock from `/state` (M5).
- [x] An Accessibility grant survives a rebuilt, re-signed build. By hand, step 4; F8
  shows the code-signing check behind it.
- [x] The hook-route tests pass under `BELLWETHER_IMPL=rust`: 20 of 20 in the black-box
  suite. The `/state` guard cases go to M3 (deviation 6).
- [x] RSS, cold start and SMAppService from the CLI are recorded below (F4, F5).

#### M1 by-hand checks (maintainer)

Use the signed build at
`shell/src-tauri/target-dev/release/bundle/macos/Bellwether.app`, built with
`--test-port 47418 --test-home /tmp/bw-m1`. `A` below is that path. Its brain uses
port 47418 and `/tmp/bw-m1`, so it never touches the repository install on 47417.
Before step 1, `/tmp/bw-m1/hook-token` must exist (one line, 0600). The developer
created it with the test token `testTokenNotReal_abcdefghijklmnop`.

1. **Logout and login with the login items.**
   1. Run `BELLWETHER_HOME=/tmp/bw-m1 "$A/Contents/MacOS/Bellwether" autostart install --write --brain-only`.
      It should print `brain agent: enabled`.
   2. Log out (Apple menu > Log Out) and log back in.
   3. Run `curl -s http://127.0.0.1:47418/health`. Expect `"name":"bellwether"` and
      `"port":47418`.
   4. Run `ps -o pid,ppid,args -p <pid from /health>`. Expect `Bellwether serve --port
      47418` with ppid 1.
   5. Run `"$A/Contents/MacOS/Bellwether" doctor --port 47418`. Expect `PASS brain`
      and `PASS login: brain`.
   6. Optional, the app login item: `… autostart install --write` (without
      `--brain-only`), log out and in again, and the strip starts. It shows no sheep
      without a brain `/state` (M5). Afterwards run `… autostart uninstall --write`.
2. **Login Items in System Settings.** While step 1's agent is registered, open System
   Settings > General > Login Items & Extensions. Under "Allow in the Background",
   Bellwether should be listed and on. Note its wording and icon for M6.
3. **The Automation prompt from a sheep click.**
   1. Start the Node brain's strip feed on another port, or use step 1's agent with
      `bellwether replay` (M5 makes this simpler). Or prove only the prompt:
      `open -n -a "$A" --args --action automation-iterm2` with iTerm2 running.
   2. Expect a prompt saying "Bellwether" wants to control "iTerm2", with the usage
      string from `Info.dist.plist`. Click Allow, and with a sheep, its tab is
      selected.
   3. Afterwards run `tccutil reset AppleEvents com.fanzootechnology.bellwether`.
4. **Accessibility survives a re-signed rebuild.**
   1. Run `open -n -a "$A" --args --demo`, then `"$A/Contents/MacOS/Bellwether" --ax-prompt`.
   2. Turn Bellwether on in System Settings > Privacy & Security > Accessibility, then
      quit the demo strip from the tray.
   3. Rebuild and re-sign: `node scripts/build-app.mjs --test-port 47418 --test-home /tmp/bw-m1`.
   4. Start `--demo` again. `shell.log` in the data folder should say "dock:
      Accessibility granted to this binary", and the sheep should stand on the Dock.
   5. Afterwards run `tccutil reset Accessibility com.fanzootechnology.bellwether`.
5. **Clean up** after any of the above:
   1. `BELLWETHER_HOME=/tmp/bw-m1 "$A/Contents/MacOS/Bellwether" autostart uninstall --write`
      should print off and off.
   2. `launchctl print gui/$(id -u)/com.fanzootechnology.bellwether.brain` should say
      `Could not find service`.

**Results (maintainer, 2026-10-13, on the notarized build):**
1. Logout and login: after login, the brain agent ran as `Bellwether serve --port
   47418` with ppid 1, started after the login. Doctor in a fresh Terminal.app window
   showed PASS for signature, token, brain and brain token, and `login: brain` enabled.
   - 1.6, the full install (`autostart install --write` without `--brain-only`):
     after a second logout and login, `login: app` was enabled and a second strip ran
     (pid 32629). The stale BTM app record from an ad-hoc copy (F7) did **not** block
     it.
   - That strip showed the maintainer's **live** flock (F10).
2. System Settings > Login Items > Allow in the Background lists "Bellwether" (one
   entry for both items) and "bellwether-shell" (the repository's legacy agent).
3. The Automation prompt for iTerm2 appeared. It was reset afterwards.
4. Accessibility survived the re-signed rebuild: the demo sheep stood on the Dock with
   no new grant. It was reset afterwards.
5. `autostart uninstall --write` printed off and off. The strip it had started kept
   running, and had to be stopped with `kill` (F11).
6. From a herdr terminal opened before the logout, doctor gave wrong answers (F9).

#### M1 findings (prove first)

**F1. SMAppService with the main executable as the agent's program works (R6).**
Shown first on the ad-hoc build, then on the signed, notarized build, macOS 26.6.2,
arm64.
- `autostart install --write --brain-only` → `brain agent: enabled`.
- `launchctl print gui/501/com.fanzootechnology.bellwether.brain` showed `state =
  running`, `program identifier = Contents/MacOS/Bellwether (mode: 2)`, `properties =
  partial import | runatload | resolve program | has LWCR`.
- `ps` showed `Bellwether serve --port 47418` with parent pid 1.
- `lsappinfo find pid=<pid>` returned nothing, so LaunchServices doesn't treat the
  agent as an app launch (no Dock icon).
- A tokened `POST /hook` got 204 with 0 bytes.
- After `kill -9`, launchd had a new pid answering at once (KeepAlive `SuccessfulExit:
  false`).
- On the signed build, doctor said `PASS identity`, `PASS signature` (Developer ID,
  hardened runtime), `PASS brain` (the Rust brain), `PASS brain token` and `PASS login:
  brain`.
- `uninstall --write` stopped it, and `launchctl print` then said `Could not find
  service`.
- `SMAppService.status` is `notFound` for a service that was never registered, not
  only for a missing plist.
- The fallback (a second executable) is not needed.

**F2. Automation through `osascript` as a child: attributed to Bellwether, prompted,
no -1743, and the entitlement is not what allows it.**
- The app was launched through LaunchServices (`open -n -a …/Bellwether.app --args
  --action automation-iterm2`) and sent `tell application id "com.googlecode.iterm2"
  to count windows` through `/usr/bin/osascript`.
- tccd logged `AttributionChain: responsible={TCCDProcess:
  identifier=com.fanzootechnology.bellwether …}, accessing={… identifier=com.apple.osascript …}`.
- It also logged `<… com.apple.osascript …> has the com.apple.private.tcc.allow-prompting
  entitlement`, then `promptPolicy = 2`, then `Handling access request:
  kTCCServiceAppleEvents … authValue: 1`, then UserNotificationCenter activated (the
  prompt). osascript then waited for an answer, until the 120 s limit.
- This was an ad-hoc copy signed **without** the apple-events entitlement, still under
  the hardened runtime. The entitlement only matters to a process that sends Apple
  Events itself, such as the in-process `NSAppleScript` fallback. It stays (no cost,
  it matches the plan, and it's needed if the locator moves in-process).
- A probe that asks for `version` (as D6 step 5 suggested) sends no Apple Event:
  AppleScript reads an app's version, name and id from its bundle. tccd logged no
  AppleEvents request for it (deviation 7).
- One prompt for "Bellwether" (the ad-hoc test copy) was raised on the maintainer's
  screen and left unanswered. `tccutil reset AppleEvents
  com.fanzootechnology.bellwether` was run afterwards.

**F3. No JIT entitlement is needed (A4). Confirmed on the signed, notarized build.**
With only the apple-events entitlement and the hardened runtime:
- `--selftest` printed `selftest: done PASS` (54 animations, 8 tinted sheets, parsed
  by WebKit's own XML parser);
- `--demo`, launched through LaunchServices, drew the flock, with `heartbeat 3, 13
  sprites, page visible` and the WebKit occlusion line in the trace.

The ad-hoc build had given the same result earlier.

**F4. The brain's footprint while linking Tauri (R14): acceptable, so one executable
stays.**

5 runs each on this Mac (M-series, macOS 26.6.2). Cold start is spawn to the first
`/health` 200, warm runs after the first. RSS is from `ps`.

| Process | Cold start, first run | Cold start, warm | RSS idle | RSS after 100 hooks |
|---|---|---|---|---|
| `Bellwether serve`, the bundle executable (links Tauri, WebKit, AppKit; 5.3 MB) | 238 ms | 5.8–10.7 ms | 10.2 MB | 10.4 MB |
| the same brain without Tauri (`crates/brain/examples/serve.rs`; 0.67 MB) | 305 ms | 4.4–5.7 ms | 2.1 MB | 2.3 MB |
| the agent started by launchd, ad-hoc build | n/a | n/a | 10.4 MB | n/a |
| the agent started by launchd, signed and notarized build | n/a | n/a | 10.4 MB | n/a |
| today's Node brain (live, read with `ps` only) | n/a | n/a | 61.2 MB | n/a |

Linking Tauri costs about 8 MB of RSS and about 3 ms of warm start. It is still about
6× smaller than the Node brain.

**F5. SMAppService works from the CLI under Terminal: no hand-off needed for
`autostart` (for M6).**
- The executable run by its own path from a terminal registered and unregistered the
  agent and the main-app item directly. `--via-app` also works.
- Through the lower-case symlink, `NSBundle.mainBundle` is not the app: the bundle id
  read as nil, and `mainAppService` reported `notFound`. So the CLI re-runs
  `autostart` from the bundle's real executable when started through a link, and
  identity reads the bundle by path. After that fix, registering through the symlink
  worked.
- `permissions …` still needs the hand-off (R7). That is unchanged and not tested
  here.

**F6. Build isolation.** `tauri build` with the overlay wrote only to `target-dev/`.
`shell/src-tauri/target/release/bellwether-shell` kept sha256 `35ceb697…ae22` and its
mtime (Oct 11 09:19) throughout. The build script checks this itself.

**F7. A login item registered from an ad-hoc build poisons the Developer ID one
(matters for M6 and M8).**

What happened, from the logs:
- At 22:06:04 macOS SIGKILLed the signed brain agent: `xpcproxy exited due to
  OS_REASON_CODESIGNING | Launch Constraint Violation … (Constraint not matched)`.
  The crash report says `termination namespace CODESIGNING code 4`. `launchctl list`
  showed `- 78`.
- The developer had registered it, from the signed build, about 3 minutes after the
  notarized build.
- Cause: at 09:29:42, when the developer first registered the agent from an **ad-hoc**
  copy, Background Task Management created item `F1BEC749…`
  (`8.com.fanzootechnology.bellwether.brain`), with a launch constraint (LWCR) taken
  from that signature.
- Unregistering doesn't delete BTM items. Every later registration logged
  `registerLaunchItem: found existing item: uuid=F1BEC749…`, including the one from
  the Developer ID build at another path. Then `xpcproxy: Launch constraint set on
  F1BEC749…`, and a Developer ID binary doesn't satisfy a constraint made for an
  ad-hoc one.
- After the failed spawn, launchd logged `Requesting LWCR update on next spawn`. At
  22:06:14 BTM logged `invalidateLaunchItem: found item with UUID F1BEC749…`.
- After `uninstall --write` and one more `install --write`, BTM used a new item,
  `AC0B132C…`, whose constraint matched. The agent ran, and `/health` answered from
  pid 90622.
- Not the cause: /Applications (the app ran from `target-dev/`), quarantine and
  translocation (none on a local build), and the plist (`BundleProgram` matches the
  program; no `AssociatedBundleIdentifiers` is needed).

Fixes made:
- `autostart install` now refuses to register a bundle that is signed ad hoc and
  carries the permanent id (tested: "refused: this build is signed ad hoc but carries
  the permanent id …", nothing registered).
- `build-app.mjs --adhoc` now builds under its own id,
  `com.fanzootechnology.bellwether.adhoc`, with agent label
  `com.fanzootechnology.bellwether.adhoc.brain`.
- Doctor explains an enabled brain agent that doesn't answer, and how to recover:
  uninstall, then install again.

For M6 and M8:
- Never register under the permanent id from anything but a Developer ID build.
- After an update that changes the signature, check that the agent comes back (M9
  measures this). If it hits a launch-constraint failure, unregister and register
  again.

**F8. Code-signing check behind TCC grants: re-signed code still matches.**
- TCC stores the client's designated requirement and checks new code against it (the
  `matchesCodeRequirement` lines in F2).
- The signed build's requirement is `identifier "com.fanzootechnology.bellwether" and
  anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */
  and certificate leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate
  leaf[subject.OU] = (TEAM ID)`. It contains no cdhash.
- A copy with different code was re-signed with the same Developer ID: CDHash
  `7ee0fe25…` against the notarized build's `3aebcd1e…`.
  `codesign --verify -R="<that requirement>"` said "explicit requirement satisfied".
- An ad-hoc build failed the same requirement ("code failed to satisfy specified code
  requirement(s)").
- So a grant should carry over to a rebuilt Developer ID build. The real grant is by-hand
  step 4.

**F9. A terminal from before the last login can't see login items, and doctor used to
misreport that.**
- The maintainer's herdr server outlived the logout. From its shells, doctor said
  `login: brain` off, "macOS has never had it registered", and `signature (unavailable)`,
  while the agent was running and enabled.
- Reproduced from the developer's shell, which was in the same state. `launchctl list`
  is empty, and `launchctl print gui/501` gives "Could not print domain: 141: Reentrancy
  avoided".
- `launchctl managername` gives "Could not get manager name." with exit 153. In a
  current session it exits 0.
- `codesign -dv` prints `Authority=(unavailable)`, because the certificate chain can't
  be read. `security find-identity` lists no identity, so this session can't sign
  either.
- `open -a` fails with `kLSNoExecutableErr`, so the `--via-app` hand-off doesn't help
  there.
- Doctor now decides the session from `launchctl managername`. In an old session it
  says, for both login items: "can't check or change login items from this terminal
  session: it predates your last login (… a terminal whose server outlived a logout,
  such as herdr or tmux). Open a new terminal window and run the command there". It
  reports Accessibility as unknown rather than not granted.
- A second check: when macOS reports the brain agent as notFound or notRegistered, but
  the brain answering on the port runs **this bundle's executable** with ppid 1 (by
  `proc_pidpath`), doctor WARNs that the terminal is probably from before the last
  login, and never says "never registered".
- `Authority=(unavailable)` is now an INFO line ("signed with a Team ID, hardened
  runtime; the certificate name is unavailable (this terminal session predates your
  last login …)"), not a WARN.
- `autostart` refuses in an old session, with the same message and exit 2. Bad usage is
  still exit 3.
- Shown from the developer's stale shell, and unit-tested (`login_verdict`,
  `signature_verdict`).

**F10. The app login item ignores the test port and home.**
- The strip started at login gets no environment and no arguments, so it uses the
  defaults: port 47417 and the default home and token. From a test build that means a
  live install's flock.
- That is correct for a real install. Test builds must not register the app item.
- Choice: refuse, rather than write test values somewhere the app reads at login (the
  app has no such config until M5's app-data layout).
- `autostart install` now refuses the app item when the bundled agent plist carries
  `--port` or `BELLWETHER_HOME` (a test build), or when `BELLWETHER_HOME` is set: "use
  --brain-only with test builds".
- It also refuses the brain agent when `BELLWETHER_HOME` is set to something other
  than the bundle's plist says, because the bundle decides.
- Unit-tested (`agent_overrides`, `install_refusal`). Not run live: the developer's
  shell is from before the last login, where `autostart` stops earlier (F9).
- Recommendation for M5/M6: the app reads its port and home only from Application
  Support, never from the environment. A test build then needs a test macOS user.

**F11. `autostart uninstall` didn't stop what the login item had started.**
- Unregistering the agent stops the brain (launchd does that). Unregistering the main
  app leaves a running strip running.
- Choice: stop it. After `uninstall --write` that removes the app item, `autostart`
  sends SIGTERM to processes whose executable (by `proc_pidpath`) is **this exact
  bundle's** and whose command line is the app (not `serve`, not the CLI, not
  `--action`). That excludes itself and its parent, never touches another copy or the
  repository's `bellwether-shell`, and waits up to 5 s.
- It prints "stopped the running strip (pid N)", or how to quit one that didn't stop.
- It reports anything of this bundle still running that it left alone, such as a
  `serve` not started by launchd. With `--brain-only` it names a running strip and
  doesn't stop it.
- Unit-tested (`is_app_mode`). Not run live, for the reason in F10.

**M1 review fixes (BR-3K: "fix first, only D1 is blocking"), 2026-10-13:**
- **D1 (blocking).** `Bellwether --help`, `-h` and `--version` started the strip on the
  default port, because every first argument starting with a dash went to the app.
  - These three now go to the CLI under any argv[0]. `dispatch` tests cover them.
  - Run live on a copy of the bundle with the new binary, with a test config as a guard:
    `--help` and `-h` print the usage, `--version` prints "bellwether 0.1.0 (rust)", and
    no strip starts.
- **D2.** The F10 and stale-session guards were only in the CLI, so the app's
  `--action main-app-register` skipped them.
  - They now live in `login_items::preflight`, which `register` and `unregister` call
    first, whichever path asks. The CLI also runs it for all items before registering
    any.
  - Unit-tested, including a check that both functions call it. Nothing was registered
    to prove it.
- **D3.** Doctor's "agent enabled but no brain answers … Launch Constraint Violation"
  fired whenever `--port` differed from the agent's port.
  - Doctor now takes the agent's own port: the plist's `--port`, else `port` in its
    home's config.json, else 47417.
  - When that differs from the port checked, it says "the brain agent listens on port
    N, not M: run `bellwether doctor --port N`", with no F7 recovery. Unit-tested.
- **D4.** With two `X-Bellwether-Token` headers, the first one won.
  - Now anything but exactly one header is a 403. Node joins repeats into "a, b",
    which never matches.
  - Pinned by a crate test, and by a black-box test that runs against both brains (21
    tests now).
- **Smaller items:**
  - `npm run test:cargo` runs `cargo test --manifest-path shell/src-tauri/Cargo.toml
    --workspace` from the root (deviation 1).
  - `npm run test:rust` doesn't build: a tester guard forbids `cargo build` in any npm
    script. Instead the black-box suite refuses a default dev binary that is older than
    the Rust sources, and says which `cargo build` to run (tried by touching a source).

**Registered and removed on this Mac**, all under `com.fanzootechnology.bellwether`:
- From ad-hoc copies, in the morning: the agent, registered and unregistered three
  times; the main-app item, registered and unregistered at once, once.
- From the signed build, in the evening: the agent, registered twice (the first time
  hit F7) and unregistered after each.

- The maintainer's by-hand run (2026-10-13) registered both items from the notarized
  build and unregistered them in step 5 (off and off). The test strip was then stopped
  by hand (F11).

At the end:
- `autostart status` said off and off;
- `launchctl print` said `Could not find service`;
- nothing listened on 47418.

Also cleaned up:
- the LaunchServices registrations of the scratch copies (`lsregister -u`);
- the AppleEvents TCC decision for the id (`tccutil reset`).

Left behind, which only the maintainer can clear:
- BTM keeps disabled item records: the agent (`AC0B132C…`, from the Developer ID
  build, harmless) and an app item `2.com.fanzootechnology.bellwether` made from an
  ad-hoc scratch copy.
- That app item might make the main-app login item from a Developer ID build hit the
  same constraint failure as F7. By-hand step 1.6 would show it.
- Clearing BTM records takes `sfltool resetbtm`. That resets every app's background
  items and needs a restart, so it was not run.

### M2. Conformance harness and CI

Builds:

- `BELLWETHER_IMPL` / `BELLWETHER_BIN` in `test/helpers.mjs`, with every spawning test
  going through it;
- the shared vector files extracted from the pure-function tests (redact, normalise,
  installer, diff, config);
- the `conformance` cargo feature with the `__` subcommands;
- the differential script;
- the parity-table CI job;
- cross-target `cargo check` of core and brain;
- `test/DIVERGENCES.md`. It starts with what M1 already diverges on:
  - a `/hook` body that hasn't arrived after 30 s gets an uncounted empty 204 from
    Rust, where Node answers 408 (deviation 5);
  - `/health` keys are written sorted (deviation 10).

**Done when:**

- `npm test` (Node) still passes all of today's roughly 836 tests unchanged in number;
- `npm run test:rust` runs and prints the parity table (most rows red, hook-route
  green);
- the Linux and Windows `cargo check` of core and brain is green;
- a release `tauri build` contains no `__` subcommands (`Bellwether __fixture` exits
  1 with "unknown command").

#### M2 status (developer, 2026-10-13): done locally; the first CI run is pending

Built:
- the switch: `spawnBrain` / `spawnBrainSync` / `brainCommand` in `test/helpers.mjs`.
  Every test that starts the brain or its CLI goes through it (17 test files, 56 call
  sites, the `/bin/sh` shims included). Under `BELLWETHER_IMPL=rust` the helper:
  - runs `$BELLWETHER_BIN` (default: the dev build, refused when older than its sources)
    with argv[0] `bellwether`, so the binary is always its CLI, never the strip;
  - turns the Node CLI's `BELLWETHER_CONFIG` / `BELLWETHER_DATA_DIR` into a temp
    `BELLWETHER_HOME` whose `config.json` carries the same test port;
  - throws on anything that would reach port 47417, and never runs `autostart`;
  - stops, by pid, any Rust process a test left running.
- the shared vectors, made by `test/fixtures/build-vectors.mjs` from the Node brain:
  `redact-vectors.json` (149), `normalise-vectors.json` (26), `config-vectors.json` (39),
  `diff-vectors.json` (23) and `installer-vectors/` (11 settings files, each with the
  bytes after install, the preview, and the bytes after uninstall). The cases came out of
  tester-m2, tester-m2-recheck, flock, brain, tester-m6 and hooks; those tests now read
  them and assert the vectors' answers too. `crates/brain/tests/vectors.rs` reads the same
  files. `npm run test:vectors` checks they are current.
- the `conformance` feature of `shell/src-tauri` and `src/conformance.rs`: `__list`,
  `__env` (working: `parse_env_value`), and `__redact`, `__normalise`, `__fixture`,
  `__json`, `__diff`, which say "not ported yet (milestone N)" and exit 1.
- `scripts/differential.mjs` (`npm run differential`): six areas, seeded generators,
  10⁵ cases each; Node answers all of them, Rust answers the ported ones over JSON lines.
- `test/conformance.mjs` (`npm run test:rust`) and `test/conformance.json`: the parity
  table, also appended to `$GITHUB_STEP_SUMMARY`.
- `.github/workflows/ci.yml`: the Node suite, the cargo tests with and without the
  feature, the parity table and the differential, the release check, and the Linux and
  Windows `cargo check`. No secrets, read-only token, actions pinned by SHA.
- `test/DIVERGENCES.md`, with deviations 5 and 10, the `dropped` counter and the refused
  `X-Pasture-*` headers, each with its pinning test; the two new tests are in
  `crates/brain/tests/http.rs`.
- `.gitattributes`, so git never converts the installer vectors' line endings.

Progress against **Done when**:
- [x] `npm test`: 958 tests, 958 pass. That is the same number as before M2; the plan's
  "about 836" was out of date.
- [x] `npm run test:rust` prints the parity table: hook-route-blackbox green under both,
  most rust cells red (see the M2 findings below).
- [x] `cargo check -p bellwether-brain --all-targets` is green for
  `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`, run here after `rustup target
  add`. There is no `crates/core` yet (deviation 16).
- [x] A release build has no `__` commands: `bellwether-shell __fixture` from `cargo
  build --release` (no features) printed `unknown command "__fixture"` and exited 1
  (deviation 18). Cargo tests pin it too.
- [ ] Only CI can prove: the workflow runs at all, the Node suite on a GitHub macOS
  runner, the cross-checks on Ubuntu, and the job summary. It runs on the first push.

#### M2 first CI run (run 38460881139) and its fixes

The release check, the cargo tests and both cross-checks passed. The Node suite and the
parity table failed, for these reasons:

- **Redaction time budgets (tester-m2 and tester-m2-recheck).** A fixed wall clock
  (1 s or 1.5 s per 5 MB input) failed on the shared runner: `userinfo` took 2481 ms,
  against 1237 ms here, where it was already near its budget. Every input grows linearly
  here (4× the input takes 3.7–4.1× the time).
  - Fix: `cleaningProblems` in test/helpers.mjs. The budgets stay as written, multiplied by
    `machineSlowdown()`. That is the time of a fixed redaction-like regex workload, which
    doesn't use the code under test, measured in the same process against this Mac's
    174–185 ms. It is never below 1, so this machine keeps the exact budgets. An input
    over budget is measured again, best of 3, before it counts.
  - Tried and dropped: a linear-growth check (time(N) / time(N/4) ≤ 8). It caught an
    injected quadratic cleaner, but under 2× CPU oversubscription the large allocations
    made linear inputs read 13–20×, a false alarm.
  - Under that load the calibrated budgets held (slowdown measured 1.64–1.79, 0
    failures). A quadratic cleaner is still caught (811 ms against a 100 ms budget).
- **THIRD-PARTY.md depended on the machine's cargo cache** (docs.test.mjs 6/7, tester-m10's
  licensing test). The parity job built the macOS binary, so `~/.cargo/registry` held only
  macOS crates. The check then ran and rendered the other platforms' crates as "not
  downloaded" (reproduced here with a macOS-only `CARGO_HOME`: same 6/7).
  - Fix in `scripts/third-party.mjs`: a crate not in the local registry keeps its
    committed row, which was read from a registry that had it. A crate with no row still
    fails the check.
  - CI runs `cargo fetch` (every platform's crates), so it verifies every row.
- **The test count** (957 on CI against 958 here). A skipped `describe` registers none of
  its tests. The horn-mask suite, skipped without the pet art, has one test, and the
  suite itself counts as one skipped test.
- **The 5 skips on CI:**
  - docs: the THIRD-PARTY check, with no cargo registry;
  - horns: the pet art;
  - tester-m10: the autostart CLI test, which needed `target/release/bellwether-shell`;
  - tester-m8: three `--locate` tests, which needed the same binary.

  Fixes:
  - CI now fetches every crate and the pinned, hash-checked pet art, and builds the strip
    (debug).
  - tester-m8 uses `BELLWETHER_STRIP_BIN`, else whichever build of the strip exists.
  - tester-m10's test runs in a temp copy of the checkout, with a stub file as the strip
    "build", the same on every machine.
- **Silent early returns made visible.** Four tests passed without checking anything when
  something was missing:
  - tester-m10's doctor walk after step 1, and the two tests that need its brain (no pet
    art);
  - tester-m2-recheck's replay of real recordings (`data/recordings` is private and never
    committed);
  - tester-isolation-recheck's cargo check (no `~/.cargo/bin/cargo`).

  They now call `t.skip` with the reason.
- **tester-m10 in the parity table:** "25/27" was 1 failure (the licensing check) plus 1
  skip, which the cell counted as a missing pass. Cells now show skips apart
  (`pass 9/9, 3 skipped`).
- **brain-server, node 20/21 in the parity job only.** It passed in the Node suite job of
  the same run, and here under 2× CPU load. The runner didn't keep the test's output, so
  which test failed is unknown. The runner now prints every failing test's name and error
  to the log and the job summary.

#### M2 findings

- **The shared `.env` reader differs from `node --env-file` in two shapes** (found by the
  differential's oracle sample). Node reads a quoted value across lines until it closes,
  and it stops reading the file at a line with no key or no `=`. `brain/envfile.mjs` and
  `settings.rs` agree with each other on 10⁵ cases but not with Node there. This matters
  only for importing a legacy token (M3); it is in DIVERGENCES.md, and the differential
  fails on any other shape.
- **A URL password containing `@` is only partly redacted.** The vector "URL credentials
  with odd characters" pins today's answer: `https://u%40x:p@ss:w0rd@h.test/` becomes
  `https://u%40x:•••@ss:w0rd@h.test/`. The Rust port must match it first (M3). Fixing it
  is a change to both, with a new vector.
- **What the parity table shows today.** `hook-route-blackbox` is green under both.
  `tester-migrate` also passes under rust, but only because an unknown command prints no
  token; the table marks such a pass `pass *`. Eleven of the files the later milestones
  name test `brain/*.mjs` in process and never start the brain, so they show "not
  wired" until their milestone runs them against the binary (through the `__` commands
  or the CLI).
- **Injected fault.** Changing one character of the Rust `parse_env_value` (`'#'` to
  `'!'`) made the differential report 4068 differences in 10⁵ cases and exit 1;
  reverted, 0 differences and exit 0. Rust redaction doesn't exist yet, so the M2 test
  plan's "one-character change in Rust redact" has to wait for M3.

### M3. Brain port A: state, redaction, `/state`, `/control`, persistence

Builds, in `crates/core` and `crates/brain`: config and defaults, the token file,
normalise, redact, flock, publisher, persist, identity and labels, `/state` long-poll,
`/control`, the CLI commands `serve`, `record`, `status [--watch] [--json]`,
`mute`/`unmute`, `show`/`hide`, `replay`, `token init|rotate`.

**Carried over from the M1 review (BR-3K):** the M1 brain accepts any number of
connections. With more than about 250 idle connections open, new hooks waited up to
the 10 s header timeout. M3 must:
- bound connections, either with a cap that closes idle ones first or with a short
  idle timeout;
- add a test that holds 300 idle sockets while a tokened hook still gets its 204
  quickly.

M4's doctor probes go through the same listener.

**Done when:**

- under `BELLWETHER_IMPL=rust`, these pass: `brain-server`, `brain`, `flock`,
  `normalise`-using suites, `privacy`, `token` (except retired `.env`-writer cases,
  listed in DIVERGENCES.md), `palette-labels`, `tester-m1`, `tester-m1-reverify`,
  `tester-m2`, `tester-m2-recheck`, `tester-m5`;
- the 21 golden fixtures match byte for byte through both the cargo test and the JS
  runner;
- the redaction and normalisation differentials show zero differences over at least
  10⁵ generated cases each;
- `replay --burst` against the Rust brain shows a `/hook` p99 no worse than the Node
  brain's on the same machine.

#### M3 status (developer, 2026-10-13): done locally; CI not yet run

Built:
- **`crates/core`** (new; deviation 24), platform-neutral, no runtime:
  - `jsstr.rs`: JS strings kept as UTF-16 code units, one `char` each, so lengths, slices,
    sort order and regular-expression bounds count code units as Node does, and a lone
    surrogate survives (deviation 25);
  - `json.rs`: `JSON.parse` / `JSON.stringify` over JS values: JS key order, JS number
    formatting, `undefined` left out, lone surrogates as `\udxxx`, a 1000-level depth limit;
  - `redact.rs` (normalise, redact, truncate, user patterns), `normalise.rs`, `config.rs`,
    `flock.rs` (identity and labels included), `fixture.rs` (the golden-fixture runner),
    `publisher.rs` (the throttle, clock-free), `persist.rs`, `token.rs`, `envfile.rs`,
    `shellstate.rs`, `status.rs`, `replay.rs` (rebuild, burst), `mask.rs`, `outfile.rs`
    (`record --out`), `cliargs.rs` (Node's strict flag rules);
  - tests: 23 unit tests, `tests/vectors.rs` (every redact, normalise and config vector),
    `tests/fixtures.rs` (the 21 fixtures, byte for byte).
- **`crates/brain`**: the state machine's task (`actor.rs`: flock, publisher, recorder,
  state.json, timers), `/state` long-poll and `/control` (`state.rs`), the connection bounds
  (`conns.rs`), a blocking client (`client.rs`), and the commands `serve`, `record`,
  `status [--watch] [--json]`, `mute`/`unmute`, `show`/`hide`, `replay` and
  `token init|rotate` (`cli.rs`). `/hook` hands events over with a `try_send` (`dropped`
  counts a full queue). New cargo tests: `/state` and `/control`, `reload-token`, the cap,
  the idle timeout (`tests/http.rs`), and `token init` / `rotate` (`tests/token_cli.rs`).
- **The `__` commands**: `__redact`, `__normalise` and `__fixture` are real; `__json` and
  `__diff` stay stubs (M4). `__session` is new: the test bridge (deviation 26).
- **The redaction fix** (maintainer's decision for M3), in both brains: a URL's userinfo
  runs from `scheme://` to the last `@` before the host (the authority ends at the first `/`
  or whitespace). Everything from the userinfo's first `:` to that `@` is redacted; the user
  name stays. It is a plain scan, `urlCredentials` in brain/redact.mjs and `url_credentials`
  in redact.rs. **A behaviour change of the Node brain**, live for the maintainer only when
  his brain restarts. The vector "URL credentials with odd characters" now expects
  `curl https://u%40x:•••@h.test/` (it pinned the leak `…:•••@ss:w0rd@h.test/`), and a new
  group "URL credentials" has 14 vectors: `@` and `:` in the password, `@` in the path and
  in the query, an email in the path after real credentials, two URLs on a line (space- and
  comma-separated), a percent-encoded `@`, an `@` in the user name, no path, a port and query
  without a path, a password past the old 500 limit, and three left alone (user name only,
  `scp` syntax, an empty password). No other vector changed. The two brains agree, so there
  is no DIVERGENCES entry. tester-m2 has a new test reading the group.
- **The connection cap** (from the M1 review; DIVERGENCES 11): at start the brain raises its
  soft descriptor limit (to at most 8192); a connection with nothing in flight that sends
  nothing for 5 s is closed; past 1024 connections (fewer under a lower limit) a new one
  closes the one idle longest, else the oldest `/state` long poll; a hook in flight is never
  closed.
- **Wiring** (`test/impl.mjs`, deviation 26): the M3 files take the brain from
  `test/impl.mjs`, which is brain/*.mjs under node and the Rust brain under rust;
  `test/fixture-runner.mjs` too, so the JS runner drives the Rust reducer. Tests that have
  nothing in the Rust brain yet say so with `notUnderRust(reason)` and count as skipped
  (deviation 27).
- CI: the cross-target `cargo check` covers `-p bellwether-core -p bellwether-brain`
  (closes deviation 16). `crates/core` is in the workspace; THIRD-PARTY.md lists
  `unicode-normalization` 0.1.25 and `tokio-macros` 2.7.2 (MIT OR Apache-2.0, MIT).

Progress against **Done when** (run on this Mac, 2026-10-13):
- [x] Under rust, all 12 files M3 names pass (parity table, `npm run test:rust`; current is
  M3, so they are required now):

  | file | node | rust |
  |---|---|---|
  | hook-route-blackbox (M1) | pass 23/23 | pass 23/23 |
  | brain-server | pass 22/22 | pass 22/22 |
  | brain | pass 32/32 | pass 32/32 |
  | flock | pass 78/78 | pass 78/78 |
  | palette-labels | pass 9/9 | pass 8/8, 1 skipped * |
  | privacy | pass 7/7 | pass 6/6, 1 skipped * |
  | tester-m1-reverify | pass 70/70 | pass 2/2, 68 skipped |
  | tester-m1 | pass 40/40 | pass 10/10, 30 skipped |
  | tester-m2-recheck | pass 67/67, 1 skipped | pass 67/67, 1 skipped |
  | tester-m2 | pass 104/104 | pass 101/101, 3 skipped |
  | tester-m5 | pass 14/14 | pass 14/14 |
  | token | pass 13/13 | pass 4/4, 9 skipped * |

  The skips under rust, every one marked `notUnderRust` with its reason:
  - M4 (doctor, hooks install/uninstall/print): palette-labels "doctor says so"; privacy
    "doctor warns about them"; token "hooks install preview…" and "a failing check names
    itself…"; tester-m1 the "installer ownership" suite and 11 doctor tests; tester-m2 the
    three hooks `--port` tests; tester-m1-reverify the D1, M1-7 and M1-8/9 suites, the two
    `hooks print --out` tests, the example-token test (doctor and hooks) and the `--hang`
    and `--slow` doctor tests;
  - retired with the Node CLI (DIVERGENCES 6): token's seven `.env` writer tests;
    tester-m1-reverify "an already-exported token wins over the .env"; the shim suites
    (tester-m1 "shim", tester-m1-reverify M1-13);
  - tester-m2-recheck's real-recordings test skips under both (data/recordings is private).
  - `*` in the rust column: the file still imports brain/*.mjs for its node-only tests.
  - This is the final run, after the review fixes below (skips counted inside skipped
    suites): node 28 of 28 files green, rust 13 of 28, all 12 required rows green, exit 0. An earlier run at load average 6–7 had one red
    node cell: brain-server's "/hook p99 under 5 ms" for the Node brain (8.6 ms, and
    5.7–6.4 ms in 3 reruns alone); the Rust brain passed it in the same run (M3 findings).
- [x] The 21 golden fixtures match byte for byte: `crates/core/tests/fixtures.rs` (cargo),
  and `test/flock.test.mjs` through `test/fixture-runner.mjs` under rust, which now also
  compares each transition line's JSON text with the file's. `__fixture`: 0 differences.
- [x] Differential, 10⁵ cases each: redact 0 differences, normalise 0 (seeds 1 and 7);
  fixture 0 (21 cases); env 0. The redact generator now also makes URL userinfo with `@`,
  `:`, `%40` and `#`, values straddling the 200-unit bounds with astral characters and lone
  surrogates, 1000-unit cookies, and JS-dialect user patterns. Injected fault (the M2 test
  plan's): `"pin"` changed to `"pim"` in the Rust secret words gave 514 differences in
  20 000 cases; reverted, 0.
- [x] `replay --burst` (release build of the Rust brain, the Node brain with the test
  preload, each on its own test port, 3 runs each), `/hook` p99:

  | events/s | client | Rust brain | Node brain |
  |---|---|---|---|
  | 200 | Rust CLI | 0.41, 0.40, 0.41 ms | 0.84, 0.64, 0.56 ms |
  | 200 | Node CLI | 0.86, 0.83, 0.84 ms | 0.97, 0.96, 1.02 ms |
  | 1000 | Rust CLI | 0.30, 0.25, 0.32 ms | 0.39, 0.38, 0.28 ms |
  | 1000 | Node CLI | 0.46, 0.44, 0.63 ms | 0.50, 0.73, 0.51 ms |

  Every answer was an empty 204. With the same client, the Rust brain's p99 is no worse.
- [x] The redaction risks D2 names: look-around becomes a match plus a check of the
  neighbours, and `(?!-)` folds into a class; `\d \w \b` are ASCII and `\s` is JS's set,
  with case-insensitive words spelled out (vectors with Arabic-Indic digits and NBSP);
  NFKC is unicode-normalization 0.1.25 (Unicode 16) against Node 22.14's ICU 76.1
  (Unicode 16); the rules run in Node's order, each left to right; truncation is by code
  point. User patterns with look-around or backreferences are exit 3 naming the pattern
  (DIVERGENCES 5).
- [x] Counts: `npm test` 963 tests, 962 pass and 1 skipped (the private recordings). That is
  958 plus five new tests: URL credentials, 300 idle sockets, the full stderr pipe,
  `record --out` into an unwritable directory, and an unusable `token init`.
  `npm run test:cargo` runs 115 tests, 117 with `--features bellwether-shell/conformance`.
  `npm run test:vectors`: all 60 files match.
- [x] The connection test: `test/hook-route-blackbox.test.mjs` holds 300 idle sockets
  against a brain started with a 256-file soft limit, then sends a tokened hook on a new
  connection: 204 at once on both brains. Run against a Rust brain with the fix switched
  off (`BELLWETHER_TEST_NO_FD_RAISE`, cap lifted) it fails with ECONNRESET.
- [x] Release check: `cargo build --release --target-dir
  shell/src-tauri/target-dev/release-check`; `__fixture` and `__session` answer
  `unknown command`, exit 1. `shell/src-tauri/target/release/bellwether-shell` kept sha256
  `35ceb697…ae22`.
- [ ] Only CI can prove: the workflow on GitHub's runners (the parity job builds the
  conformance binary and runs the M3 rows as required), the Windows and Linux checks there
  (they pass here with both targets installed), and the timing tests on a shared runner.

#### M3 review fixes (BR-3K: "ship", 2026-10-13)

- **A stderr nobody read froze the Rust brain** (BR-3K defect 1: at about 64 KB of log,
  `/hook` and `/health` timed out). Logging now goes through a queue of 1024 lines per sink
  (stderr, brain.log) to a writer thread; a full queue drops the line and counts it in
  `/health` `counters.logDropped` (DIVERGENCES 3). The serving thread never writes a log
  line itself. At exit the queues get 500 ms to drain. New black-box test, both brains:
  3000 hooks with stderr piped and never read, each an empty 204 (slowest under 1 s), and
  `/health` answering with `accepted` up by 3000; under rust `logDropped > 0`. With the old
  blocking write put back, the test hangs until its timeout.
- **Unlisted differences** (BR-3K defect 2):
  - `serve`/`record --port`, `bellwether version` and the help text are listed as
    DIVERGENCES 13, with pinning tests.
  - `record --out` into a directory that can't be written: **the Node brain changed**. It
    now exits 3 with `cannot open <file>: EACCES` instead of a stack trace. The Rust brain
    gives the same line, and now opens the recording before binding the port, as Node
    does. One test runs against both brains (brain-server.test.mjs).
  - `token init` with a token that is there but unusable (for example `short`): **both CLIs
    changed to match**.
    - The Rust one used to replace it silently. The Node one left it alone and said
      "already set".
    - Now both leave it alone, name the problem ("isn't a usable token (shorter than 16
      characters, or not all of [A-Za-z0-9_-])") and exit 3. `--force` replaces it, says so,
      and never prints the token. `--force` never replaces a usable token.
    - `token init --force` is new in the Node CLI too. One test runs against both brains
      (token.test.mjs). The unit tests are `token.rs`, `init_writes_once_and_imports`.
- **Skipped suites hid their size** (BR-3K observation 3). The parity table's rust column
  now counts every test the node run of the file has and the rust run didn't run.
  tester-m1 reads "pass 10/10, 30 skipped" and tester-m1-reverify "pass 2/2, 68 skipped".
  With `--impl rust` alone it falls back to TAP's count and says so.
- **The "/hook p99 under 5 ms" test** (BR-3K observation 2) judges the best of up to three
  2 s bursts:
  - p99 under 5 ms times `machineSlowdown()` (exactly 5 ms on this Mac), and p50 under
    2 ms, in that burst;
  - every burst must still have no failures and at most 4 snapshots a second;
  - it is the same test for both brains.
- **Redaction: two known gaps, pinned, not changed** (BR-3K observation 1). Both brains
  have them, and both predate M3:
  - an unencoded `/` or whitespace in a URL password ends the authority before the `@`,
    so `https://u:pa/ss@h.test/` leaks;
  - an `@` in a query that follows host:port with no `/` is read as userinfo, so
    `http://localhost:3000?email=me@x.io` becomes `http://localhost:•••@x.io`.

  New vector group "URL credentials, known gaps" (3 vectors) pins today's output. tester-m2
  checks it, and so do the cargo vectors.
  - **Follow-ups:** decide whether a `/` that is followed by `…@host` with no other `/`
    can be a password character, and whether `?`/`#` should end the authority (which would
    leak a raw `#` or `?` in a password).

#### M3 findings

- **Node's own p99 test is load-sensitive.** brain-server's "/hook p99 under 5 ms" holds
  for the Rust brain (debug build, optimised server crates: 1.4–2.2 ms on the same JS
  client) but failed for the Node brain at load average 6–7 (5.7–8.6 ms). It is the M2 CI
  run's unexplained "brain-server, node 20/21" again. Not changed here; a calibrated budget
  like the redaction tests' (`machineSlowdown`) would be the fix.
- **The M1 fd problem, measured:** a Rust brain started with a 256-file soft limit and
  neither fix reached 259 descriptors with 300 idle sockets open, and a new hook got
  ECONNRESET (5 of 5 runs). With the limit raised, 204 in 6 ms.
- **Node's hook path vs Rust's:** at 200 events/s the Rust brain's median is lower too (p50
  0.19–0.21 against 0.21–0.26 ms with the Rust CLI as client; 0.31–0.35 against
  0.39–0.43 ms with the Node CLI), with the state machine, logging and state.json on the
  same thread. Early versions had spikes: the built-in regexes compiled on
  the first hook, and state.json saved with fsync; both fixed (deviation 30).

### M4. Brain port B: hook installer, doctor, migrate, pets

Builds:

- the lossless JSON module;
- the installer (install, uninstall, print, `--out` guard, install record, backups,
  changed-while-editing check);
- the diff;
- `doctor` with the new checks: token-file mode, login items, legacy agents,
  `onFailure`, WebKit flag, identity, legacy headers; the `node` check removed;
- `pets import`;
- `migrate` (the repository-to-repository behaviour of today, ported).

**Done when:**

- under rust, these pass: `hooks`, `doctor`, `docs` (the hook parts), `shellctl` and
  `tester-shellctl` (whatever still applies), `tester-m3`, `tester-m6`, `tester-m8`,
  `tester-m10`, `tester-regression`, and the migrate suites;
- the JSON differential shows zero unexplained differences over at least 10⁵ generated
  settings files;
- for every installer vector, install followed by uninstall returns the original bytes;
- `hooks install` previews are byte-identical to Node's on the same inputs;
- a doctor run against a settings file with a Bellwether entry carrying
  `onFailure: "block"` FAILs.

#### M4 status (developer, 2026-10-14): done locally; CI not yet run

Built:
- **`crates/core`**:
  - `lossless.rs`: the lossless JSON of R2 (number literals verbatim, file key order, a
    duplicate key's first place with its last value, `__proto__` an ordinary key,
    `JSON.stringify` escaping over unit strings, `serialise()`'s indent and EOL detection,
    the 10-character gap cut, `deepEqual`);
  - `diff.rs`: the LCS diff, line for line from brain/diff.mjs;
  - `hooks.rs`: the hook block, install, uninstall, `shapeProblems`,
    `assertOthersPreserved`, `countGroups`, the install record, `planEdit` and `applyPlan`
    (backup 0600 with `wx`, the hash check, the temp file and rename, the advisory record);
    `EVENTS` now lives here (deviation 44);
  - `settings_checks.rs`: doctor's checks of settings files and the other settings levels,
    `globMatch`, and the new `onFailure: "block"` FAIL;
  - `outfile.rs`: the `hooks print --out` guard joins `record --out`'s;
  - tests: `tests/installer.rs` (every installer vector and every diff vector) and unit
    tests (round trips, shapes, ownership, layouts, the changed-while-editing check).
- **`crates/brain`**: `doctor.rs` (the brain-side checks, `next`, `--e2e`, the report and
  `--json`; deviation 33), `pets.rs` (`pets import`), `migrate.rs` (deviation 35), and in
  `cli.rs` the commands `hooks print|install|uninstall`, `pets import`, `migrate`, and
  `token rotate`'s step 2 (deviation 36).
- **The shell's `cli.rs`**: `doctor [--json] [--e2e] [--settings] [--port]` around the brain's
  checks, with identity, signature, app data, login items, Accessibility, F9's wording and
  review D3's agent port kept; `shell` (deviation 38); the help.
- **The `__` commands**: `__json` and `__diff` are real; `__session` has the installer,
  the diff and doctor's pure checks; `runDoctor` runs the binary (deviation 40).
- **The Node brain**: doctor FAILs `onFailure: "block"` too (one change, deviation 33).
- **The harness**: a temp HOME for the Rust binary and the real-settings guard (deviation
  39); `test/impl.mjs` has the installer, diff and doctor; every M4 file is wired; Node-only
  tests skip with their reason (deviation 41); test/DIVERGENCES.md 14–21.

Progress against **Done when** (this Mac, 2026-10-14):
- [x] Under rust, every file M4 names passes, and so do isolation, envload, rename and pets
  (added to M4, deviation 42). The parity table (`npm run test:rust`, current M4, the final
  run; exit 0; `*`: the file also tests brain/*.mjs in process; every skip says why):

  | file | node | rust |
  |---|---|---|
  | hook-route-blackbox (M1) | pass 23/23 | pass 23/23 |
  | brain-server | pass 22/22 | pass 22/22 |
  | brain | pass 32/32 | pass 32/32 |
  | flock | pass 78/78 | pass 78/78 |
  | palette-labels | pass 9/9 | pass 9/9 |
  | privacy | pass 7/7 | pass 7/7 |
  | tester-m1-reverify | pass 70/70 | pass 62/62, 8 skipped |
  | tester-m1 | pass 40/40 | pass 36/36, 4 skipped |
  | tester-m2-recheck | pass 67/67, 1 skipped | pass 67/67, 1 skipped |
  | tester-m2 | pass 104/104 | pass 104/104 |
  | tester-m5 | pass 14/14 | pass 14/14 |
  | token | pass 13/13 | pass 6/6, 7 skipped * |
  | docs | pass 8/8 | pass 8/8 |
  | doctor | pass 33/33 | pass 32/32, 1 skipped * |
  | envload | pass 7/7 | pass 1/1, 6 skipped * |
  | hooks | pass 73/73 | pass 72/72, 1 skipped * |
  | isolation | pass 6/6 | pass 6/6 * |
  | migrate | pass 22/22 | pass 3/3, 19 skipped * |
  | pets | pass 5/5 | pass 1/1, 4 skipped * |
  | rename | pass 21/21 | pass 9/9, 12 skipped * |
  | shellctl | pass 9/9 | pass 1/1, 8 skipped * |
  | tester-m10 | pass 27/27 | pass 5/5, 22 skipped * |
  | tester-m3 | pass 17/17 | pass 8/8, 9 skipped * |
  | tester-m6 | pass 20/20 | pass 20/20 |
  | tester-m8 | pass 12/12 | pass 12/12 |
  | tester-migrate-recheck | pass 17/17 | pass 2/2, 15 skipped * |
  | tester-migrate | pass 25/25 | pass 5/5, 20 skipped * |
  | tester-regression | pass 27/27 | pass 24/24, 3 skipped |
  | tester-shellctl | pass 26/26 | pass 12/12, 14 skipped * |

  node 29 of 29 files green, rust 29 of 29; every M3 skip with an M4 reason is gone (palette-
  labels, privacy, token, tester-m1, tester-m1-reverify, tester-m2 run their doctor and
  installer tests against the Rust binary). The skips left are the repository's own
  (deviation 41) and the private recordings.
- [x] The JSON differential: 0 unexplained differences in 10⁵ generated settings files
  (seed 1); the only explained ones are integer-like keys in file order (DIVERGENCES 16).
  Injected fault: writing the literal `-0` as `0` gave 933 unexplained differences in 20 000
  cases; reverted, 0. The diff area: 0 differences in 10⁵.
- [x] Installer vectors: install then uninstall gives back the original bytes for all nine
  vectors Node round-trips (deviation 34, for the other two); `hooks install` previews are
  byte-identical to Node's for ten of eleven, through the CLI (hooks.test.mjs, "every
  installer vector through the CLI") and in `crates/core/tests/installer.rs`; the eleventh
  differs only in its integer-like keys' order.
- [x] Doctor FAILs a bellwether entry with `onFailure: "block"`, exit 3, under both brains
  (doctor.test.mjs, "onFailure (plan D3)"); by hand on the Rust CLI:
  `FAIL  hook onFailure  bellwether entries on PreToolUse set "onFailure": "block": with the
  brain down, Claude Code would block every tool call there. …`, then `2 check(s) failed
  (exit 3).`
- [x] Counts: `npm test` 984 tests, 983 pass and 1 skipped (the private recordings), from 963:
  21 new tests (the eleven installer vectors through the CLI, `onFailure`, the real-settings
  guard, `pets import`, two migrate CLI tests, three migrate previews, `PASTURE_HOOK_TOKEN`
  alone, the hook commands in `help`); no assertion was weakened (tester-regression's
  check of the real settings file now uses its metadata, deviation 39). `npm run
  test:cargo` runs 134 tests, 136 with `--features bellwether-shell/conformance` (115 and 117
  at M3). `npm run test:vectors`: all 60 files match. `third-party.mjs --check`: up to date
  (no new crate).
- [x] Differential (seed 1, 10⁵ each): env 0, redact 0, normalise 0, json 0 unexplained
  (22 336 explained), diff 0, fixture 0 (21); `node --env-file` sample 0 unexplained. Seed 7:
  json 0 unexplained, diff 0.
- [x] Cross-target: `cargo check -p bellwether-core -p bellwether-brain --all-targets` is
  green for `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`, no warnings.
- [x] Release check: `cargo build --release --target-dir shell/src-tauri/target-dev/release-check`;
  `__json`, `__diff`, `__session` and `__fixture` answer `unknown command`, exit 1.
  `shell/src-tauri/target/release/bellwether-shell` kept sha256 `35ceb697…ae22`.
- [ ] Only CI can prove: the workflow on GitHub's runners with M4 required in the parity
  job, the Windows and Linux checks there, and the timing tests on a shared runner.
- Not run: a real `doctor --e2e` (one `claude -p` API call; the fake `claude` covers the
  path), `migrate --write` against a real repository install or real LaunchAgents (never on
  this Mac), and `pets import` of a real eSheep file (data/pets is not read here).

#### M4 findings

- **The guard found a real read.** tester-regression hashed `~/.claude/settings.json` to
  prove it untouched; the new guard refuses reads, so it now compares the file's metadata
  (deviation 39). No test reads or writes the real file any more.
- **Load-sensitive timing, again.** Two earlier full runs each had one red brain-server
  cell, at load averages of 8–20 from programs outside this work (a game and another
  session's test runs): once under rust ("/hook p99 under 5 ms"), once under node (its own
  sender managed only 137 of 200 events in a second). Alone, and in the final run at a
  load of about 5, both passed 22 of 22. The M3 finding's fix (a calibrated budget for the
  snapshot test too) still stands.
- **The 8000-line settings file** (tester-m1-reverify) takes a 272 MB LCS table in both
  brains, as brain/diff.mjs does; it stays well inside the test's 15 s.
- **`hooks install` across a token change moves bellwether's group to the end** of each
  event's list (it removes the stale group and appends a fresh one), in both brains. That is
  today's behaviour; `token rotate` uses it as it is.

#### M4 review fixes (BR-3K: "fix first, short list", 2026-10-14)

- **D1, `token rotate`.** The hook entries change only when the brain on the port confirmed
  the switch, or when no brain answers at all. A brain that answers and refuses (the Node
  brain answers `reload-token` with 400 "unknown action") gets the old token back in the
  file, the entries untouched, exit 1, and a message saying why. After a failed settings
  write the second `reload-token` is checked; the message says whether the brain, the file
  and the entries agree again or which one is off. New `crates/brain/tests/rotate_cli.rs`: a
  refusing brain, no brain, a failed write, and a failed switch back (fake brains on free
  ports). **Recommendation: the Node brain should not learn `reload-token`.** It reads the
  repository's `.env`, not the app's token file the Rust CLI rotates, so a switch would be
  meaningless; refusing is the correct answer, and the Node brain goes in M11.
- **D2, the real-settings guard.** The preload compares the device and inode of every read
  or write target with the real `settings.json` (it never reads it), and a normalised name
  (the `/System/Volumes/Data` firmlink dropped, lower case) for a file that doesn't exist
  yet, such as `settings.local.json`. It now also wraps `fs.openAsBlob`, `new fs.ReadStream`,
  `new fs.WriteStream` and `process.binding('fs')` (any path argument naming a protected file
  stops the call). `rustEnv` recognises the real HOME by inode, so a firmlink or upper-case
  spelling is replaced too, and `brainCommand` refuses any argument (`--settings`, `--out`,
  `--settings=…`, a relative path against its cwd) that names the real files. Two new tests
  in isolation.test.mjs try each of BR-3K's bypasses on a decoy file the test protects the
  same way (`protectForTest`); none touches the real file.
- **The live-port guard and port spellings.** The helper reads every `--port` as a number
  (leading zeros, `+`, spaces) for every command, `hooks` included, and refuses 47417 in any
  spelling. Both CLIs now take a port only as a plain decimal: `serve`, `record`, `replay`
  and `doctor --port` refuse `047417`, `+47417`, ` 47417` and `4.7417e4` (exit 3), as `hooks
  --port` already did. Node's `replay --port` changed the same way. Tested in hooks.test.mjs
  ("a port is one spelling") and the Rust `ports` unit test; tester-m2's list gained
  `047500`, `+47500` and ` 47500`.
- **D3, the differential.** The json area's "explained" now uses BR-3K's model: the
  integer-like keys are renamed to plain ones, Node answers, the keys are renamed back, and
  Rust must equal that byte for byte (escapes, numbers, layout and the keys' order). Seed 1:
  0 unexplained, 22 336 equal to the model. BR-3K's three faults injected through a proxy:
  `é` written as `\u00e9` 13 323 unexplained, upper-case `\u001F` 3 351, DEL as `\u007f`
  3 433 (all three were 0 before).
- **D4, `pets import`.** Elements nest at most 64 levels (a million nested is refused at
  once, "nested more than 64 deep", where it overflowed the stack); duplicate attributes are
  found with a set (100 000 attributes check in milliseconds, 16.7 s before); the title and
  the proposed map are printed with control characters and bidi overrides as `?`. The Node
  CLI prints nothing from an imported file (it has no `pets import`), so there is nothing to
  match there. Tests: `pets.rs` `hostile_files_are_refused_quickly_and_nothing_prints_raw`,
  and pets.test.mjs through the CLI.
- **D5.** (a) Depth: listed as DIVERGENCES 22, with a test under both brains (Node installs
  into a file 1100 levels deep, Rust refuses it with exit 3 and writes nothing). (b) Lone CR:
  `diff.rs` keeps a CR that no LF follows, as `split(/\r?\n/)` does; diff-vectors.json gained
  four lone-CR cases, and the differential's diff generator makes lone CRs inside lines, as
  lines and at the end (0 differences in 10⁵).
- **D6.** DIVERGENCES 21 has a pinning test (each brain's wording); deviation 35 and
  DIVERGENCES 17 now say the ping runs only when HOME is the real home, and that otherwise
  the entries are rewritten directly. The ping-then-rewrite path is tested without the real
  HOME: `migrate::Options` has `check_brain` (the CLI sets it from the real-home check), and
  `crates/brain/tests/migrate_cli.rs` runs it against a Rust brain that takes the token,
  one that holds another, and none. The exotic vector's CLI test under rust now asserts the
  installed bytes, the preview (Node's diff of exactly those bytes) and the uninstalled bytes.
  Four more migrate tests run through the CLI under both brains (the preview's steps, `--from`
  a data directory with `--force`, `--env` with a corrupt state.json and a `--from` that isn't
  an install, and no token in any `.env`, where the app takes the entries' token): Rust
  migrate now has 12 tests through the CLI plus 3 cargo tests (against Node's 26).
- **D7, `onFailure` at the other levels.** A bellwether entry with `"onFailure": "block"`
  in a project, local or managed settings file FAILs (`hook onFailure`, exit 3, naming the
  file and the level), in both doctors. Tested in doctor.test.mjs, both the check and a whole
  run.
- **D8, the timing tests.** The throttle tests' lower bounds (more than 300 events, at least
  7 snapshots) and the throughput test's (at least 2 snapshots) scale with
  `machineSlowdown()`, which a starved attempt measures again; a starved attempt is retried,
  and three in a row make the test skip as inconclusive, not fail. The upper bound (at most 4
  snapshots in any second) is checked on every attempt, unscaled.

After the fixes (this Mac, load about 4–6):
- `npm test`: 994 tests, 993 pass, 1 skipped (the private recordings). Ten new tests.
- `npm run test:cargo`: 137; with the conformance feature, 139.
- `npm run test:rust`: exit 0, node 29 of 29, rust 29 of 29 (hooks 75/75 + 1 skipped,
  doctor 33/33 + 1, isolation 8/8, migrate 7/7 + 19).
- `npm run differential`: env 0, redact 0, normalise 0, json 0 unexplained (22 336 equal to
  the model), diff 0, fixture 0, `node --env-file` 0 unexplained.
- `npm run test:vectors`: all 60 files match. `third-party.mjs --check`: up to date. The
  cross-target check of core and brain: green for Linux and Windows, no warnings.

### M5. Shell on the Rust brain, app-data layout

Builds:

- the shell reading `hook-token` and config from Application Support and logging to
  `~/Library/Logs/Bellwether`;
- the app spawning `Bellwether serve` as a child when no brain answers;
- `get_pet` from bundled resources and imported pets;
- version check against `/health` with an agent restart;
- the Node spawn path deleted from the shell.

**Done when:**

- on a machine without Node in PATH, the installed `.app` shows sheep for a live
  Claude Code session, alerts with a bubble, lambs, and click-to-focus;
- the README's "Things to check by hand" items 1–11 pass with the app;
- the strip capability still lists exactly the six permissions (test);
- killing the app with `kill -9` stops the brain child within a second; a launchd brain
  survives it.

#### M5 status (developer, 2026-10-14; maintainer by-hand checks in part, 2026-10-15): done locally; live sessions shown by hand; some by-hand checks still open

Built:
- **Two layouts in the shell** (`shell/src-tauri/src/settings.rs`; deviation 45):
  - **App**, when the executable is inside an app bundle: home `BELLWETHER_HOME`, else a
    test build's bundled home, else `~/Library/Application Support/Bellwether`; the config
    loaded and checked as the Rust brain loads it (`bellwether_core::config::load`, every key
    optional); the token from `BELLWETHER_HOOK_TOKEN` or `hook-token` (checked; read again
    while waiting for one, and when the brain answers 403); `shell-state.json` and imported
    pets in the home; the log at `~/Library/Logs/Bellwether/shell.log`, or `<home>/logs/` for
    any other home.
  - **Repository**, for any binary outside a bundle (`target/release/bellwether-shell`,
    `target-dev/`): the checkout's `config.json`, the token from the environment or `.env`
    (the Node CLI's rule; the `.env` is now read only when the environment has no token),
    `data/`, `data/shell.log`. Unchanged from M4, so a rebuilt repository strip keeps working
    against the Node brain.
  - A test build (`--test-port/--test-home`) also gives its **strip** the bundled port and
    home, and refuses a different `BELLWETHER_HOME` (deviation 46).
- **The brain child** (`brain.rs`): when no brain answers, the strip starts its own executable
  as `serve --port <port>`, with `BELLWETHER_PARENT_PID`, the strip's home (and in the
  repository layout its config and token), stdout and stderr discarded (the brain writes
  `brain.log`). Stopped on quit with SIGTERM, then SIGKILL after 2 s; restarted at most once
  every 10 s if it dies. The brain's watchdog looks every 250 ms (`crates/brain/src/lib.rs`).
  **The Node spawn path is gone**: no `brain.nodePath`, no `brain/cli.mjs`, no
  `brain-from-shell.log` (a cargo test checks brain.rs for them).
- **The version check** (`brain.rs`, `check_version`, app layout only): at start the strip
  reads `/health`. A bellwether brain of another version that is this bundle's login agent
  (pid running this bundle's executable, ppid 1) is restarted with `launchctl kickstart -k
  gui/<uid>/<label>`, and the strip waits up to 10 s for the new version. Any other brain is
  left alone with a warning (deviation 49).
- **`get_pet`** (`lib.rs`, `PetPlace`): in the app, a bundled pet
  (`Contents/Resources/pets/<name>/animations.xml`, pinned by the `pets.lock.json` compiled in)
  else an imported one (`<home>/pets/<name>/`, read once and checked against
  `pets.user.lock.json` by the new `bellwether_brain::pets::load`); `pets.default` names it,
  `bellwether` by default (deviation 47). The repository keeps `data/pets` and its pins.
- **`shell start|stop|restart`** for the app (`cli.rs`; deviation 48, DIVERGENCES 19).
- **The M4 follow-up**: `pets::printable` also turns U+061C, U+200B and U+FEFF into `?`.
  Found while running it: `pets import` printed the proposed map with its line breaks as `?`
  (the whole pretty JSON went through `printable`); it now goes line by line (deviation 51).
- **Tests**: cargo `the_strip_capability_lists_exactly_the_six_permissions` (the capability's
  exact list and order, strip windows only, no other capability file, and the handler list,
  build.rs and the autogenerated permissions naming the same six),
  `the_app_loads_imported_pets_by_their_pin_and_never_a_path`,
  `the_version_check_restarts_only_this_apps_own_agent`,
  `the_child_is_this_executable_as_serve_marked_with_the_parent`,
  `the_app_home_is_the_environment_or_the_test_builds_or_the_default`,
  `the_repository_reads_the_env_file_only_when_the_environment_has_no_token`, the sanitiser
  cases in `pets.rs`, and the strip commands outside a bundle in `cli.rs`; Node: shellctl.test
  "`shell stop` outside the app" (both brains), and pets.test pins the map's line breaks.
- README: the repository strip's brain (section 6) and "The signed app" (the strip, logs,
  imported pets, `shell start|stop|restart`, test builds).

Proved on a signed test bundle (`node scripts/build-app.mjs --test-port 47431 --test-home
<scratch>/bw-m5`, Developer ID, hardened runtime, not notarized; codesign `--verify --deep
--strict` "valid on disk / satisfies its Designated Requirement"; entitlements only
`automation.apple-events`). Every run: `env -i` with `PATH=/usr/bin:/bin:/usr/sbin:/sbin` (no
`node` on it), a temp `HOME`, `BELLWETHER_HOME=<scratch>/bw-m5` with `config.json` on port
47431. The token was made by the bundle's `token init`, the pet (the pinned eSheep file,
downloaded fresh, sha256 checked) by the bundle's `pets import`.
- **The strip on its own brain.** With nothing on 47431, the strip logged `started the brain:
  …/Bellwether.app/Contents/MacOS/Bellwether serve --port 47431 (pid 5878)`; `ps` showed that
  pid as the strip's child; `pet esheep64 loaded: 54 animations`; WebKit occlusion off.
- **Flock, bubble, lambs.** Three sessions replayed by the bundle's own `replay` (fixtures s2
  to its permission prompt, s21 to its lamb's permission prompt, s5 to its first lamb): every
  `/hook` an empty 204; `status` showed three sheep, an ALERT with its text, six lambs (one
  ALERT, the parent "a lamb needs you", "+1 more"), one lamb on the third. The strip's own
  `--snapshot` PNG shows the bubble over the first sheep, the lambs with the "lamb" tag, "+1"
  and the lamb's bubble, and the third sheep with its lamb.
- **Quit stops the child.** SIGTERM to the strip: `stopping the brain the shell started (pid
  5878)`; the brain logged `SIGTERM, stopping` and saved state.json; both gone, the port free.
  Started again, the same sessions came back in the same colours.
- **`kill -9` of the app stops its brain child within a second: 97 ms** (`the app that started
  this brain (pid 6235) is gone; stopping`).
- **A launchd brain survives it.** `autostart install --write --brain-only` from the test
  build: `brain agent: enabled`, `Bellwether serve --port 47431` with ppid 1. The strip then
  logged `brain on port 47431: version 0.1.0, pid 6328 (this app's login agent)` and started no
  child. After `kill -9` of the strip the agent (same pid, ppid 1) still answered `/health`.
- **`shell start|stop|restart`** from the bundle's CLI, agent up: `started the strip (pid
  6412)`; again: `the strip is already running (pid 6412)`; `restart`: `stopped the strip (pid
  6412)` / `restarted the strip (pid 6424)` (which found the agent); `stop`: `stopped the strip
  (pid 6424)`; again: `the strip was not running`. The agent answered throughout.
- **The version check and the agent restart.** With the 0.1.0 agent running, the bundle was
  rebuilt with the brain crate at 0.1.1 (temporarily; reverted after) and its strip started:
  `the brain on port 47431 is this app's login agent at version 0.1.0 (pid 6328); this app is
  0.1.1: restarting it (launchctl kickstart -k gui/501/com.fanzootechnology.bellwether.brain)`,
  then `the login agent's brain is now version 0.1.1 (pid 6909), 76 ms after the restart`;
  pid 6909 had ppid 1.
- **Doctor** from the bundle (temp HOME, an empty test settings file): PASS identity,
  signature (Developer ID, hardened runtime), config, token (0600), brain (`the Rust brain
  (Bellwether)`), brain token, webkit (read from the app's `shell.log`), pets (`esheep64,
  imported, sha256 matches pets.user.lock.json`), login: brain; the one FAIL is the empty
  settings file's missing hooks.
- **Cleanup**: `autostart uninstall --write --brain-only` → `brain agent: off`; `autostart
  status` off and off; `launchctl print gui/501/com.fanzootechnology.bellwether.brain` →
  `Could not find service`; nothing listening on 47431–47433.
- **Click-to-focus, as far as it goes here.** `--locate` from the signed bundle with this
  session's real terminal (herdr pane, iTerm2 session) ran the herdr CLI without Node, and
  found no window: on this Mac the herdr client's process chain ends at iTerm2's
  `iTermServer` (parent launchd), not at the iTerm2 process that owns the window, so without
  AppleScript (off in the test config, so no Automation prompt was raised on the maintainer's
  screen) the herdr and process-tree strategies have nothing to go on. `locator.rs` is
  unchanged in M5. The real click is by-hand check 3 and 12–13 below.

The repository layout (how a rebuilt repository strip stays safe; deviation 45):
- The release build of this source (`cargo build --release --target-dir
  shell/src-tauri/target-dev/release-check`) is outside any bundle, so it takes the repository
  layout. Started like the LaunchAgent (`--no-spawn-brain`) against a fake checkout with a
  Node brain on a test port, it logged `starting: repository layout, agent dir …`, then quit:
  its id is `local.bellwether`, the same as the live strip's, so the single-instance plugin
  handed off to the **live** strip (pid 32594, untouched otherwise; it has probably logged one
  line, "another bellwether shell tried to start; this one keeps running", in the
  repository's `data/shell.log`). A second `local.bellwether` strip can't run beside the
  live one, so that run wasn't repeated. The layout's loading is the M4 code (tester-m8 runs
  it under `npm test` through `--locate`).
- The brain the repository strip would start (this binary as `serve --port 47433` with
  `BELLWETHER_HOME=<checkout>/data`, `BELLWETHER_CONFIG=<checkout>/config.json`, the token in
  the environment, `BELLWETHER_PARENT_PID`) accepted the repository's `config.json`, answered
  a tokened hook with an empty 204, wrote `data/state.json` and `data/logs/brain.log`, and left
  148 ms after its parent got `kill -9`.

Progress against **Done when**:
- [x] On a machine without Node in PATH, the `.app` shows sheep for a live Claude Code session, a
  bubble, lambs and click-to-focus. Shown with recorded sessions on the signed test bundle with
  Node off PATH (above), then by the maintainer with real sessions (by-hand results below: checks
  1, 3, 6). It was a test build on port 47418, not an installed release build; click-to-focus was
  shown in iTerm2 only (herdr and Terminal.app, checks 12–13, are still open).
- [ ] The README's by-hand checks 1–11 with the app: 1, 3, 6 and 8 passed (below); 2, 4, 5, 7, 9,
  10 and 11 are still open.
- [x] The strip capability still lists exactly the six permissions: cargo
  `the_strip_capability_lists_exactly_the_six_permissions` (and the Node source scans).
- [x] `kill -9` of the app stops its brain child within a second (97 ms; 148 ms for the
  repository binary), and a launchd brain survives it (the test build's agent, above).

Counts and checks (this Mac, 2026-10-15, after the M5 review fixes below; every run under
`caffeinate -dims`):
- `npm test`: 997 tests, 996 pass, 0 fail, 1 skipped (the private recordings). 994 at M4, plus
  shellctl's "`shell stop` outside the app", docs.test's fence check and its build-app check.
- `npm run test:cargo`: 146 passed, 0 failed (137 at M4); with
  `--features bellwether-shell/conformance`: 148 (139 at M4).
- `npm run test:rust` (conformance binary rebuilt first): exit 0, node 29 of 29 files green,
  rust 29 of 29 (rename 21/21 and 9/9 + 12 skipped; shellctl 10/10 and 2/2 + 8; docs 10/10
  and 10/10).
- `npm run differential` (seed 1): env, redact, normalise and diff 0 differences in 10⁵ each;
  json 0 unexplained (22 336 explained, integer-like keys); fixture 0 (21); `node --env-file`
  0 unexplained.
- `npm run test:vectors`: all 60 vector files match. `third-party.mjs --check`: up to date.
- `cargo check -p bellwether-core -p bellwether-brain --all-targets` for
  `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`: clean, no warnings.
- Release check (`--target-dir shell/src-tauri/target-dev/release-check`): `__fixture`,
  `__json`, `__diff`, `__session` print `unknown command`, exit 1.
  `shell/src-tauri/target/release/bellwether-shell` kept sha256 `35ceb697…ae22` throughout.
- An earlier rerun (2026-10-15, before `caffeinate -dims`) ran while the Mac was in
  maintenance sleep on battery; its two Node-only failures (the shim's version test, a
  redaction time budget) were sleep, and are green above.

Not done or not proved:
- a live Claude Code session and a real sheep click against the app (by-hand checks);
- the repository-layout strip window against a Node brain (blocked by the live strip's single
  instance, above);
- `Foreign` brains (another version that isn't this app's agent) are only unit-tested;
- a notarized build, and the update gap under the real updater (M9).


#### M5 review fixes (BR-3K: "ship", 2026-10-15)

- **D1, the README fence.** "``` The brain reads its token…" was a closing fence with text
  on it, so the signed-app section rendered as one code block. Fixed; a new docs.test case
  checks every fence in README, NOTICE, THIRD-PARTY.md, the plan and DIVERGENCES.md (a closing
  fence bare, an opening one a single language at most, none left open). With the old line
  put back it fails naming `README.md:614`.
- **D2, the `.env` test.** It now makes the `.env` a FIFO with no writer and calls `token_from`
  on a thread with a 3 s limit. With the read moved above the environment's early return
  (scratch mutant) it fails: `token_from blocked: it opened the .env although the environment
  has the token: Timeout`; restored, it passes.
- **D3, `shell start` after a hand-off.** A strip pid counts as started only if it is still
  alive 1.5 s after it appeared; one that quit is reported: "the strip started (pid N) but quit
  at once. Most likely another strip on the same port holds the single-instance lock …",
  exit 2. Unit-tested (`start_failure`, and that the liveness check comes before the success
  line). Shown on a rebuilt signed test bundle (port 47431): with a strip running from a copy
  of it (`Copy.app`, same id and port), `shell start` from the original printed `the strip
  started (pid 95640) but quit at once. Most likely another strip on the same port holds the
  single-instance lock …`, exit 2, and the copy's log has `another bellwether shell tried to
  start; this one keeps running`.
- **D4, the switch-over text**: step 1's doctor runs without `--port` (47417); step 3.2 says the
  settings file doesn't change and to stop if the preview shows a diff; step 5.3 says what to do
  on "requires approval".
- **D5, a leftover test bundle.** `build-app.mjs` deletes the app an earlier build left before
  building, checks afterwards that the bundled agent plist carries exactly this build's test
  flags (none for a release build), and ends with `TEST BUILD (…): for testing only; never
  install or ship it` or `release build: no test port or home in the bundle`. Switch-over step
  1 says to check that line. `testFlags`/`bundleMatches` are tested in docs.test.mjs.
- **O6, single-instance per port** (deviation 52). The lock is `/tmp/<identifier>_si.sock`; the
  strip now sets the runtime identifier to `<id>.p<port>` for any port but 47417, and leaves it
  as it is on 47417. So a rebuilt repository strip still meets the running repository strip
  (any earlier build, the Oct 11 one included) on `/tmp/local_bellwether_si.sock`, and the
  installed app keeps `/tmp/com_fanzootechnology_bellwether_si.sock`, while a test bundle
  (47418, 47431…) or a dev strip on a test port gets a lock of its own. Only Tauri's
  path-resolver folders (unused by the shell), a core command no webview is granted and the
  Linux GTK app id read that identifier; macOS still sees the bundle id. Cargo test
  `the_single_instance_lock_is_per_port_and_unchanged_on_the_live_port`. On the rebuilt test
  bundle, its strip held `/tmp/com_fanzootechnology_bellwether_p47431_si.sock` beside the live
  strip's `/tmp/local_bellwether_si.sock` (listed only), and removed it on quit. No strip with
  the id `local.bellwether` was started. The by-hand checklist's
  line about sharing the screen now says why that works.
- **O7, "own agent".** Before `kickstart -k`, the strip asks `launchctl print gui/<uid>/<label>`
  for the agent's pid and restarts only when it equals the `/health` pid (as well as running
  this bundle's executable with ppid 1). A hand-started `serve` orphaned to ppid 1 is left alone,
  and the warning says launchd doesn't list it. Cargo test `the_agents_pid_is_launchds_own`
  (the top-level `pid = N` line only; a stopped service has none).
- **O8, the sanitiser** (`pets::unprintable`): every Cc (`char::is_control`), every Cf code point
  of Unicode 16 (a range table: soft hyphen, Arabic and Syriac marks, the Mongolian vowel
  separator, U+200B–200F including ZWNJ and ZWJ, bidi overrides and isolates, U+2060–2064,
  U+206A–206F, BOM, the interlinear annotations, the Egyptian, shorthand and musical format
  controls, the tag characters), Zl and Zp (U+2028, U+2029), and the invisibles that aren't
  Cf (U+034F, the Hangul fillers, U+17B4–17B5, U+2800, the rest of the tag block). BR-3K's
  list and `A B\u{E0041}C` are tested; accents, CJK, Arabic letters, combining accents, the
  emoji variation selector and NBSP stay.
- **O9, unusable tokens.** Doctor's `next` now follows the token line's cause: none at all →
  `bellwether token init`; there but unusable → `bellwether token init --force`, then `hooks
  install --write`; others can read the file → the `chmod`; a bad environment value → fix or
  unset it (`token_next`, tested). The strip's log no longer says "no hook token: no hook
  token:", nor adds "`bellwether token init` makes one" after the token module's own advice.
  On the test bundle with `hook-token` set to `short`: doctor `FAIL token`, and `next` says
  "replace the unusable hook token: `bellwether token init --force`, …".

#### M5 by-hand checks (maintainer, later): the README's checks with the app

Use a test build so nothing touches the live install: `node scripts/build-app.mjs --test-port
47418 --test-home /tmp/bw-m5` (no `--notarize` needed), `A=shell/src-tauri/target-dev/release/bundle/macos/Bellwether.app`,
`B="$A/Contents/MacOS/Bellwether"`, and in every command `BELLWETHER_HOME=/tmp/bw-m5`.

0. **Prepare.**
   1. `mkdir -p /tmp/bw-m5 && chmod 700 /tmp/bw-m5`, and write `/tmp/bw-m5/config.json` with
      `{"port": 47418, "pets": {"default": "esheep64", "animationMap": {"esheep64": …}}}` (the map
      from `config.example.json`).
   2. `BELLWETHER_HOME=/tmp/bw-m5 "$B" token init` and
      `BELLWETHER_HOME=/tmp/bw-m5 "$B" pets import data/pets/esheep64/animations.xml --name esheep64`.
   3. `BELLWETHER_HOME=/tmp/bw-m5 "$B" hooks print --out /tmp/bw-m5/hooks.json` (the hooks for
      `claude --settings`, port 47418, never your settings file).
   4. Start the strip: `BELLWETHER_HOME=/tmp/bw-m5 "$B" shell start`. Its brain is a child
      (no agent registered). Your live strip (the repository's, or after the switch-over the
      installed app's) stays up: the single-instance lock is per port, so the test strip on
      47418 runs beside it and its sheep share the screen. Don't run a second copy of the test
      bundle on 47418: it would hand over and quit (`shell start` says so).
   5. Start sessions with `env PATH=/usr/bin:/bin:/usr/sbin:/sbin:$(dirname $(command -v claude)) claude --settings /tmp/bw-m5/hooks.json`
      (Node off PATH for the app; claude itself may still need its own runtime, which is fine:
      the app must not).
1. **Two sessions in two iTerm2 tabs**: two differently coloured sheep above the Dock, each
   near its own slot.
2. **Click-through**: a Finder icon or terminal text behind the strip, between two sheep,
   gets the click.
3. **Click a sheep**: it hops, typing still goes to your terminal; the session's tab comes
   forward. The first click raises an Automation prompt naming Bellwether (allow it).
4. **Quit and restart** (`"$B" shell restart` with `BELLWETHER_HOME`): the sheep come back in
   the same colours and slots.
5. **Full screen** (green button): the strip isn't on that Space.
6. **A lamb**: ask a session for an Explore subagent that runs `ls` in a scratch directory. A
   lamb labelled `Explore` appears beside its sheep and goes back in when it finishes; if it
   needs permission, the lamb alerts and the parent wears the "lamb" tag. A permission prompt
   in the main session shows a bubble.
7. **Capture checks**, with a sheep waiting on a permission prompt: Cmd-Shift-3 and
   Cmd-Shift-5; a QuickTime screen recording; Zoom, Teams and Meet sharing as in the README.
   Note whether the strip is visible in each.
8. **Tray** (the test strip's menu-bar sheep): Mute (sheep vanish, `"$B" status` shows MUTED,
   survives a restart), Hide the strip (survives a restart), Show labels, Quit Bellwether (the
   strip quits and its brain child stops: `curl -s http://127.0.0.1:47418/health` gets nothing).
9. **Full screen again** with Safari or Terminal: not on that Space; back when you swipe back.
10. **Fade and clicks**: the cursor near a sheep fades it; clicking it still hops; clicking
    beside it reaches what's underneath.
11. **Two monitors**: a session's terminal on the second monitor gets a strip there; dragging
    it moves the sheep within a minute; the strip goes when no sheep live there.
12. **Click inside herdr** (README 12): the hosting iTerm2 window comes forward with the
    pane's tab selected (needs the Automation grant from step 3).
13. **Terminal.app** (README 13): a click asks for Automation for Terminal, then brings the
    right window forward.
14. **Afterwards**: `"$B" shell stop` (with `BELLWETHER_HOME`), `tccutil reset AppleEvents
    com.fanzootechnology.bellwether`, and delete `/tmp/bw-m5`.

**Results (maintainer, 2026-10-15).** A test build on port 47418 with home `/tmp/bw-m5`; real
Claude Code sessions started with Node off PATH and `--settings /tmp/bw-m5/hooks.json`. Every
check that was run passed:
- 1: two sessions gave two sheep in two colours;
- 6: a permission prompt gave an alert with a bubble, and an Explore subagent showed a lamb;
- 3: clicking a sheep brought its iTerm2 tab forward, after an Automation prompt naming
  Bellwether;
- 8: the tray's Mute, Hide and Quit worked, and Quit stopped its brain child.

The AppleEvents grant was reset afterwards (step 14).

Still open (not run by hand): 2 click-through, 4 restart keeping colours and slots, 5 and 9 full
screen, 7 capture and sharing, 10 fade and clicks, 11 two monitors, 12 a click inside herdr, 13
Terminal.app.

#### M5 switch-over: moving the live repository install to the app by hand (not run)

For the maintainer, when he chooses to, before M8 automates it. It keeps the same token and
URL, so running sessions keep working; the only gap is the second between steps 5.2 and 5.3,
where a hook may print one "hook error" (decision 11).

1. **A release build of the app** (no test flags): `node scripts/build-app.mjs --notarize`.
   It deletes whatever app an earlier build left (a test build included) before building, and
   its last line must read `release build: no test port or home in the bundle`; if it says
   `TEST BUILD`, stop. Then `ditto shell/src-tauri/target-dev/release/bundle/macos/Bellwether.app /Applications/Bellwether.app`.
   Set `A=/Applications/Bellwether.app` and `B="$A/Contents/MacOS/Bellwether"`. Check
   `"$B" version` and `"$B" doctor` (no `--port`: it checks 47417, where the Node brain
   answers; `login: brain` and `login: app` should be off).
2. **Back up**: `cp ~/.claude/settings.json ~/.claude/settings.json.before-app`, and note
   `launchctl list | grep local.bellwether`.
3. **Token and data** (nothing running changes yet):
   1. `"$B" token init --from ~/Documents/projects/bellwether/.env`: `hook-token` gets the same
      token the hooks carry.
   2. `"$B" migrate --from ~/Documents/projects/bellwether` (preview), then the same with
      `--write`: config.json without the obsolete keys, state.json (colours, slots, mute),
      shell-state.json. It lists the two LaunchAgents and leaves them alone. **The token and
      the port stay the same, so nothing in `~/.claude/settings.json` changes**: the preview
      should show no hook diff. If it shows one, stop and compare it with step 2's backup.
   3. `"$B" pets import ~/Documents/projects/bellwether/data/pets/esheep64/animations.xml --name esheep64`,
      and check that `~/Library/Application Support/Bellwether/config.json` has `pets.default`
      `"esheep64"` and its `animationMap` (copy them from the repository's config.json if not).
4. **Doctor before the swap**: `"$B" doctor`. Expect PASS for config, token (0600), brain (the
   Node brain still), brain token, hooks; WARN `legacy agents` naming `local.bellwether.brain`
   and `.shell`.
5. **Swap** (do 5.2 and 5.3 back to back):
   1. `launchctl bootout gui/$(id -u)/local.bellwether.shell` (the repository strip quits).
   2. `launchctl bootout gui/$(id -u)/local.bellwether.brain` (the Node brain stops; 47417 is
      free).
   3. `"$B" autostart install --write`: `brain agent: enabled` (the Rust brain starts on 47417
      at once) and `app at login: enabled`. If either says **requires approval** (waiting for
      you in System Settings > General > Login Items), the Node brain is already stopped, so
      act at once: turn Bellwether on under "Allow in the Background" there and run
      `"$B" autostart status` again, or run rollback step 9 to bring the old install back.
   4. `curl -s http://127.0.0.1:47417/health`: a `pid` whose `ps -o ppid,args -p <pid>` is
      `1 Bellwether serve`.
   5. `"$B" shell start`: the strip; its `shell.log` (`~/Library/Logs/Bellwether/`) says `brain
      on port 47417: version 0.1.0, pid … (this app's login agent)`.
6. **Keep the old agents from coming back at login**: `mkdir -p ~/Library/Application\ Support/Bellwether/legacy`
   and `mv ~/Library/LaunchAgents/local.bellwether.brain.plist ~/Library/LaunchAgents/local.bellwether.shell.plist ~/Library/Application\ Support/Bellwether/legacy/`.
7. **Check**: `"$B" doctor` (PASS brain as the Rust brain, brain token, hooks, login: brain and
   app; no legacy agents); an existing Claude Code session's next prompt moves its sheep, in its
   old colour.
8. **Accessibility** (sheep on the Dock): `open -n -a "$A" --args --ax-prompt`, turn Bellwether
   on in System Settings > Privacy & Security > Accessibility, then `"$B" shell restart`. The
   old `bellwether-shell` entry there can be removed.
9. **Rollback**, if anything is wrong: `"$B" autostart uninstall --write` (both items off, the
   strip stopped, the Rust brain stopped by launchd); move the two plists back to
   `~/Library/LaunchAgents/`; `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/local.bellwether.brain.plist`
   and the same for `.shell`. The hooks never changed, so nothing else is needed; the app's
   folder can stay.

Why the repository install is safe until then: a strip built from the checkout (`bellwether
shell build --release`) is outside a bundle, so it reads the checkout's `config.json`, the
token from the `.env` and `data/`, exactly as before M5, and with `--no-spawn-brain` (the
LaunchAgent's argument) it never starts a brain. Without that argument and with no brain
answering, it starts its own binary as the Rust brain on the checkout's files, never Node.

### M6. Login items and legacy agent detection

Builds: `autostart install|uninstall|status` on SMAppService (directly or through the
action hand-off, per M1's finding), `requiresApproval` handling, and the startup check
and tray line for legacy `local.bellwether.*` and `local.pasture.*` agents.

**Done when:**

- `bellwether autostart install --write` and then `status` shows `enabled` for both;
- a reboot brings both up;
- `uninstall --write` removes both from Login Items;
- with a `local.bellwether.brain` plist loaded, the tray and doctor name it;
- no file is ever written to `~/Library/LaunchAgents` by the app (test with a temp
  HOME).

#### M6 status (developer, 2026-10-15): done locally, except what needs a release build or a reboot; CI not yet run

Built:
- **`autostart`** (`shell/src-tauri/src/cli.rs`, `login_items.rs`; SMAppService directly, per
  F5, with `--via-app` kept): the preflight guards of M1 review D2 stay on every path (CLI and
  `--action`), and so do F7's ad-hoc refusal, F10's test-build refusal and F11's strip stop.
  New:
  - outside Bellwether.app, `install` and `uninstall` refuse with exit 2 before any preview
    ("not running from Bellwether.app … nothing was registered or written"); they used to
    preview "would register (now unavailable)" (deviation 54);
  - `requiresApproval`: the line says what to do (System Settings > General > Login Items &
    Extensions, "Allow in the Background", and that it won't start until then); `install
    --write` and `status` then offer to open the pane, asking at a terminal and printing the
    command otherwise; `install --write` exits 2 while an item waits (deviation 54);
  - `autostart settings` opens that pane, and `--via-app` hands it to the app (action
    `login-items-settings`) (deviation 53);
  - doctor's `login: brain` / `login: app` lines show `requiresApproval` as a WARN with the
    same advice (unchanged M1 code, now tested for it).
- **Legacy agents** (`crates/brain/src/legacy.rs`, new; deviation 55): the four labels of D5.
  One counts when its plist is in `~/Library/LaunchAgents` (loaded or not), or when launchd has
  the label loaded in this user's GUI domain (plist deleted or not). launchd is asked with
  `launchctl print gui/<uid>/<label>` (read-only), and an answer counts only if its first line
  is exactly that target, so another user's domain never matters; it is asked only when HOME is
  the real home. Doctor's `legacy agents` WARN names each one with its state
  (`local.bellwether.brain (loaded, pid N)`, `… (plist only: launchd loads it at your next
  login)`, `… (loaded, pid N; its plist is gone, so it stops at logout)`).
- **The tray** (`tray.rs`): at the app's start (app layout only; not the repository strip,
  `--demo` or `--selftest`) one greyed line naming the legacy agents, and a line per login item
  waiting for approval that opens the Login Items pane (deviation 58). Both go to `shell.log`
  (`tray: notice: …`).
- **No writes to `~/Library/LaunchAgents`**: `shell/src-tauri/tests/no_launchagents.rs` runs the
  real binary (a development build, outside a bundle) under a temp HOME through every
  `autostart` form, `token init`, `hooks install/uninstall --write`, `doctor`, `migrate
  --write`, `shell stop` and the five login-item actions, with the folder missing and with
  three legacy plists in it, and checks it unchanged (entries, bytes, mtimes; not created when
  missing). With a scratch mutant that writes a plist from `autostart`, it fails ("empty:
  ~/Library/LaunchAgents changed"). `legacy.rs` and `tray.rs` have source checks that they only
  read (deviation 59).
- **The harness** (`test/helpers.mjs`): `autostart` now runs under rust for a development
  binary, which can't reach SMAppService; a `BELLWETHER_BIN` inside a `.app`, and `autostart
  settings`, are still refused (deviation 56). tester-regression's "autostart install without
  --write…" runs under both brains; rename.test has a rust test for both names' plists.
  test/DIVERGENCES.md 23.

Proved on a signed test bundle (`node scripts/build-app.mjs --test-port 47431 --test-home
<scratch>/bw-m6`: Developer ID, hardened runtime, not notarized; codesign "valid on disk /
satisfies its Designated Requirement"; entitlements only `automation.apple-events`; the build
ended `TEST BUILD (brain agent on port 47431, …)`). Every command with `BELLWETHER_HOME=<scratch>/bw-m6`
and the real HOME, from a current login session (`launchctl managername`: Aqua):
- before: `autostart status` → `brain agent: off …`, `app at login: off …`;
- `autostart install --write` → `refused: this is a test build (its brain agent runs on port
  47431), but the app started at login gets no environment … Use --brain-only with test builds`,
  exit 3 (F10);
- the app's own side, launched through LaunchServices (`open -n -g -a … --args --action
  main-app-register`): its outcome file says `"ok":false`, `app at login: refused: this is a test
  build …` (M1 review D2, on the action path);
- `autostart install --write --brain-only --via-app` → `brain agent: enabled (by the app)`, exit 0;
  `autostart status` → `brain agent: enabled`, `app at login: off`; `/health` on 47431 from pid
  49274, `ps`: `Bellwether serve --port 47431` with ppid 1; `launchctl print`: `state = running`,
  `program identifier = Contents/MacOS/Bellwether (mode: 2)`;
- `doctor --settings <scratch>/settings.json`: `PASS login: brain`, `INFO login: app … off`, and
  `WARN legacy agents  local.bellwether.brain (loaded, pid 25449), local.bellwether.shell
  (loaded, pid 32594): the repository version's login items (~/Library/LaunchAgents). A legacy
  brain and this app's brain fight over one port; …` (the maintainer's live agents, read with
  `launchctl print` only);
- the tray: `shell start` (the test strip on 47431, its own single-instance lock beside the live
  strip's) logged `warning: legacy agents: local.bellwether.brain (loaded, pid 25449),
  local.bellwether.shell (loaded, pid 32594); …`, `tray: notice: Legacy agents:
  local.bellwether.brain (running), local.bellwether.shell (running) (see bellwether doctor)` and
  `tray: 1 notice(s), Mute, Hide the strip, Show labels, Quit` (no "no tray icon" warning); then
  `brain on port 47431: … (this app's login agent)`. `shell stop` → `stopped the strip`; the
  launchd brain kept answering. The menu itself was not looked at (no screenshot, no
  Accessibility scripting on the maintainer's screen);
- `autostart uninstall --write --brain-only` (the direct path) → `brain agent: off`; `status` off
  and off; `launchctl print gui/501/com.fanzootechnology.bellwether.brain` → `Could not find
  service`; pid 49274 gone; nothing listening on 47431;
- `ls -laT ~/Library/LaunchAgents` before and after: identical (the two `local.bellwether.*`
  plists still Oct 11 09:11:09; no new file).

Progress against **Done when**:
- [ ] `autostart install --write`, then `status` shows `enabled` for both. The brain item: shown
  above (both paths, test build). The app item: a test build refuses it (F10), so only a release
  build can show it; unit tests cover the paths (`install_refusal`, `every_path_gets_the_same_checks`,
  `waiting_for_approval_says_what_to_do_and_offers_the_pane`). By-hand step 3 below.
- [ ] A reboot brings both up. Not done here (it would restart the maintainer's Mac). M1's
  logout/login brought the brain agent and, with a notarized build, the app item up (M1 by-hand
  results 1 and 1.6). By-hand step 5 below.
- [ ] `uninstall --write` removes both from Login Items. The brain item: shown above (`Could not
  find service`). Both, with the System Settings listing: by-hand step 6.
- [x] With a `local.bellwether.brain` plist loaded, the tray and doctor name it: above, with the
  maintainer's live agents (doctor's line and the strip's tray notice, from its log). The other
  shapes (a plist not loaded, loaded with its plist deleted, `local.pasture.*`, another user's
  domain, a temp HOME) are unit-tested in `crates/brain/src/legacy.rs` and by rename.test under
  rust.
- [x] Nothing is written to `~/Library/LaunchAgents` by the app: `no_launchagents.rs` under a
  temp HOME (and its mutant), and the live run above.

Counts and checks (this Mac, 2026-10-15, every run under `caffeinate -dims`):
- `npm test`: 998 tests, 996 pass, 0 fail, 2 skipped (the private recordings, and rename.test's new
  rust-only doctor test). 997 at M5, plus that test.
- `npm run test:cargo`: 154 passed, 0 failed (146 at M5: `legacy.rs` 4, tray 1, cli 2,
  `no_launchagents.rs` 1); with `--features bellwether-shell/conformance`: 156 (148 at M5).
- `npm run test:rust` (conformance binary rebuilt first): exit 0, node 29 of 29 files green, rust
  29 of 29 (tester-regression 25/25 + 2 skipped, was 24/24 + 3: the autostart test now runs;
  rename 10/10 + 12 skipped).
- `npm run differential` (seed 1): env, redact, normalise and diff 0 differences in 10⁵ each; json
  0 unexplained (22 336 explained, integer-like keys); fixture 0 (21); `node --env-file` 0
  unexplained.
- `npm run test:vectors`: all 60 files match. `third-party.mjs --check`: up to date (no new crate).
- `cargo check -p bellwether-core -p bellwether-brain --all-targets` for `x86_64-unknown-linux-gnu`
  and `x86_64-pc-windows-msvc`: clean, no warnings.
- Release check (`--target-dir shell/src-tauri/target-dev/release-check`): `__fixture`, `__json`,
  `__diff`, `__session` print `unknown command`, exit 1.
  `shell/src-tauri/target/release/bellwether-shell` kept sha256 `35ceb697…ae22` throughout.

Not done or not proved:
- the app login item registered, listed in System Settings, brought up by a reboot, and removed
  (release build only; by-hand steps below);
- `requiresApproval` live: macOS gave `enabled` at once here, so the advice, the exit code, the
  terminal offer and the tray's approval line are unit-tested only, and `autostart settings`
  (it opens System Settings) was not run;
- the tray line seen in the menu (only its log line was read);
- CI.

#### M6 by-hand checks (maintainer, with a release build: during the switch-over or M8)

A test build can't do these (F10). Use the release build of the M5 switch-over (step 1:
`node scripts/build-app.mjs --notarize`, last line `release build: no test port or home in the
bundle`, copied to `/Applications`), `A=/Applications/Bellwether.app`, `B="$A/Contents/MacOS/Bellwether"`,
in a terminal opened after your last login (not a herdr pane from before it, F9). Steps 1–5 fit
into the switch-over. Step 6 removes the app's login items and stops the brain: **do not run it
except as the switch-over rollback** (or on a spare account).

1. **Before anything moves** (after switch-over step 4): `ls -laT ~/Library/LaunchAgents > /tmp/la-before.txt`.
   `"$B" doctor` should show `WARN legacy agents  local.bellwether.brain (loaded, pid <the Node
   brain>), local.bellwether.shell (loaded, pid <the repository strip>)`.
2. **The tray line** (optional, before the swap): `"$B" shell start`. Two strips now draw the
   same flock for a moment. Click the app's menu-bar sheep: its first line is greyed, `Legacy
   agents: local.bellwether.brain (running), local.bellwether.shell (running) (see bellwether
   doctor)`. Then `"$B" shell stop`.
3. **Both items** (switch-over step 5.3): `"$B" autostart install --write` prints `brain agent:
   enabled` and `app at login: enabled`, exit 0. Then `"$B" autostart status` shows `enabled`
   twice. If either says **waiting for your approval**, the command exits 2 and asks whether to
   open System Settings (default no): type y, turn Bellwether on under "Allow in the Background", and run
   `status` again (both `enabled`). Note which happened. Note also whether the strip started at
   once from the app item (M1 showed only the brain agent starting at registration); if not,
   switch-over step 5.5 starts it.
4. **System Settings > General > Login Items & Extensions**: Bellwether under "Allow in the
   Background" (on), and Bellwether under "Open at Login" (the app item). Note the wording.
5. **Reboot** (Apple menu > Restart), log in, open a new terminal:
   1. `curl -s http://127.0.0.1:47417/health`: `"name":"bellwether"`; `ps -o ppid,args -p <its pid>`:
      `1 /Applications/Bellwether.app/Contents/MacOS/Bellwether serve`.
   2. The strip is up by itself: sheep above the Dock for any running session, and `ps -axo
      pid,ppid,args | grep "[/]Applications/Bellwether.app/Contents/MacOS/Bellwether$"` shows it.
   3. `"$B" doctor`: `PASS login: brain`, `PASS login: app`, and no `legacy agents` line (the old
      plists were moved in switch-over step 6).
   4. If the brain doesn't answer and Console shows "Launch Constraint Violation" (F7: the stale
      ad-hoc app record), run `"$B" autostart uninstall --write`, then `install --write`, and
      note it.
6. **Removal.**

   **DO NOT RUN, except as the switch-over rollback (step 9) or on a spare user account.** It
   stops the brain your hooks use: every Claude Code session prints hook errors until something
   answers on 47417 again.

   `"$B" autostart uninstall
   --write` prints `brain agent: off` and `app at login: off`, and `stopped the running strip
   (pid N)`. `launchctl print gui/$(id -u)/com.fanzootechnology.bellwether.brain` says `Could not
   find service`; System Settings no longer lists Bellwether as on. Then either finish the
   rollback, or `install --write` again.
7. **Nothing written**: `ls -laT ~/Library/LaunchAgents | diff /tmp/la-before.txt -` shows only
   what you did by hand (the two `local.bellwether.*` plists moved away in switch-over step 6);
   no file from the app.


#### M6 review fixes (BR-3K: "ship", 2026-10-15)

BR-3K added `crates/brain/tests/tester_m6_legacy.rs` (hostile plist shapes: malformed, a dangling
symlink, a directory, a FIFO, lookalike names, mode 000, an unsearchable folder; hostile
`launchctl print` answers; temp and symlinked homes). It runs in `npm run test:cargo` (a
workspace member's integration test).
- **The harness hole (deviation 56).** `inBundle` used `path.resolve`, so a `BELLWETHER_BIN`
  symlinked to `Bellwether.app/Contents/MacOS/Bellwether` passed as a development binary, while
  the binary canonicalises its own path and would have registered real login items. It now uses
  `fs.realpathSync` (an unresolvable path counts as a bundle), behind an exported `refuseRust`
  that `brainCommand` calls. isolation.test, "a symlink to Bellwether.app/Contents/MacOS/Bellwether
  is refused": BR-3K's repro, a chain of links, the bundle path itself, a missing path; with
  `path.resolve` put back it fails (`Missing expected exception: autostart install --write`).
- **By-hand step 6** is now a bold "DO NOT RUN, except as the switch-over rollback" line.
- **The pane offer defaults to no** (`[y/N]`): only `y` or `yes` (any case, trimmed) opens it;
  Enter, Ctrl-D (end of input) and anything else don't. Tested (`wants_settings`).
- **No token in the agent's home** (deviation 60): registering the brain agent is refused.
- **`launchctl print` is bounded**: 2 s (`LAUNCHCTL_TIMEOUT`), then the child it started is
  stopped by its handle and the answer is unknown: with a plist, doctor says "launchd didn't say
  whether it is loaded (launchctl print didn't answer within 2 s)"; without one, an INFO
  `legacy agents` line names the labels it couldn't check. Tested with `/bin/sleep 30` (stopped
  at 0.3 s).
- **An unsearchable LaunchAgents folder** gives a WARN `legacy agents` line, "can't read
  ~/Library/LaunchAgents (…): a repository LaunchAgent there … can't be seen", for a folder that
  can't be listed (modes 000, 300) or searched (600). `legacy::unreadable`, tested. `scan`
  itself is unchanged (BR-3K's test pins that a loaded agent still counts there).

After the fixes (this Mac, 2026-10-15, under `caffeinate -dims`):
- `npm test`: 999 tests, 997 pass, 0 fail, 2 skipped (998 before: isolation's symlink test).
- `npm run test:cargo`: 162 passed, 0 failed; with `--features bellwether-shell/conformance`: 164
  (154 and 156 before: BR-3K's 4, `legacy.rs` 3, `login_items.rs` 1).
- `npm run test:rust` (conformance binary rebuilt): exit 0, node 29 of 29, rust 29 of 29
  (isolation 9/9 under both).
- `npm run differential` (seed 1): env, redact, normalise, diff 0 differences in 10⁵ each; json 0
  unexplained (22 336 explained); fixture 0 (21); `node --env-file` 0 unexplained.
- `npm run test:vectors`: all 60 files match. `third-party.mjs --check`: up to date.
- The cross-target check of core and brain (Linux, Windows): clean, no warnings (the brain crate
  rechecked).
- Release check: `__fixture`, `__json`, `__diff`, `__session` print `unknown command`, exit 1.
  `shell/src-tauri/target/release/bellwether-shell` kept sha256 `35ceb697…ae22`.
- Not rerun live after the fixes: the test bundle (the token refusal, the 2 s bound and the
  unreadable-folder line are unit-tested; the live run above predates them and had a token).

### M7. Onboarding window

Builds: the window, the `onboarding` capability with the 11 commands, steps 2–8 of D6,
and the tray "Set up Bellwether…".

**Done when:**

- on a fresh user account, the steps take a new user from first launch to a sheep for
  a new Claude Code session without a terminal;
- the hook diff shown equals `bellwether hooks install` output for the same file,
  token masked;
- editing settings.json between preview and Install makes Install refuse and
  re-preview;
- the capability test asserts the exact permission lists of both capabilities;
- calling an onboarding command from the strip webview (devtools in a debug build)
  fails;
- refusing every optional permission leaves a working strip;
- Import… with a DOCTYPE file shows the refusal.

### M8. Migration to the app

Builds: `bellwether migrate --to-app` and onboarding step 1 (D9), `--undo`, and the
legacy discovery for both `local.bellwether.*` and `local.pasture.*`.

**Done when:**

- for each of three prepared temp-HOME fixtures (renamed repo install, pre-rename
  install, hooks-only install), the preview lists exactly the expected actions and
  `--write` produces the expected Application Support contents, settings bytes and
  no legacy plists in `~/Library/LaunchAgents`;
- a second run says "nothing to migrate";
- `--undo` restores the original settings bytes and plists;
- on a real repository install (manual, maintainer), sheep keep their colours and
  slots across the move, and a session started before the migration keeps showing
  alerts after it;
- a forced failure (port held by a dummy listener) leaves the old setup running and
  says what holds the port.

### M9. Release pipeline, updater, Homebrew

Builds: `release.yml` and `ci.yml` per D7, the updater wiring and tray item, the DMG
notarization step, `SHA256SUMS`, and the tap cask with an automated PR.

**Done when:**

- tag `v0.2.0-rc.1` produces a draft release with a stapled DMG (`spctl --assess --type
  open --context context:primary-signature` accepted), the updater pair and
  `latest.json`;
- an installed rc.1 updates itself to rc.2 through "Check for Updates…", keeps its
  Accessibility and Automation grants, and its brain reports the new version within 2 s
  (gap under 1 s, measured with a 50 ms `/health` poll);
- `brew install --cask <owner>/tap/bellwether` installs it, and `bellwether doctor`
  works from PATH;
- a PR from a fork runs `ci.yml` and has no access to the `release` environment;
- the job log contains no secret values (grep for the cert password, the key id and
  the issuer).

### M10. Original sheep art

Builds: SPEC.md, the artist's delivery, `pet.json`, `scripts/build-pet`, the bundled
`bellwether` pet and its pin, the horn and ΔE tests on the real sheet, and the NOTICE
and THIRD-PARTY updates.

**Done when:** acceptance checks (a)–(g) in D8 pass, and the default config shows the
new sheep with no `pets fetch`.

### M11. Remove the Node brain, ship 1.0

Builds:

- the deletion of `brain/*.mjs`, the Node shim and `bellwether.cmd` (the conformance
  suite now runs only against Rust);
- a README rewritten for the app (install, onboarding, CLI from PATH, building from
  source as a contributor section);
- a release-notes section on migration.

**Done when:**

- the D2 deletion gates are met;
- `npm test` contains only player and dev-script tests plus the conformance suites
  (all green against Rust);
- `cargo test --workspace` is green;
- `docs.test.mjs` is updated and green;
- tag `v1.0.0` ships through M9's pipeline.

## Test plan

| Milestone | What the tester verifies | How, without touching a live account |
|---|---|---|
| M1 | signature, notarization, entitlements, agent survival, guards | `codesign`/`spctl`/`stapler`; POSTs with crafted headers to `/hook`; the conformance hook-route subset. Uses a test account or temp `BELLWETHER_HOME`; Login Items checked on a spare macOS user account |
| M2 | the harness itself | runs both impls; inject a deliberate one-character change in Rust redact and confirm the differential and parity table catch it |
| M3 | state model and redaction parity | golden fixtures, vectors, differential; `replay` of fixtures into a Rust brain on a non-default port (`--port`) |
| M4 | installer fidelity and doctor | only temp copies of `test/fixtures/settings-sample.json` and generated files (`--settings <tmp>`); never `~/.claude/settings.json`. Doctor runs with `--settings` |
| M5 | full app behaviour | the manual checklist; `--demo` and `replay --tag` for scripted sheep without real sessions; a real session only with `claude --settings <tmp hooks file>` from `hooks print --out` |
| M6 | login items | spare macOS user account; temp HOME tests assert nothing is written to LaunchAgents |
| M7 | onboarding | spare user account; the settings file there is disposable; capability tests in cargo |
| M8 | migration | three fixture HOMEs built by a script (plists, `.env`, `data/`, settings) with launchctl calls stubbed, as today's tests do when HOME isn't the real home; the one real run is manual and preceded by a backup |
| M9 | pipeline | rc tags on a draft release; the updater tested against a `latest.json` on a draft or a local static server via an endpoint override available only in debug builds |
| M10 | art | automated checks plus the snapshot grid review |
| M11 | nothing regressed | the full suite; a clean install on a fresh account; one migration run |

Fixtures and offline modes the developer must provide:

- `BELLWETHER_HOME` and test HOME support everywhere, with launchctl and SMAppService
  stubbed when HOME isn't real (the existing pattern);
- the `conformance` feature;
- the shared vector files;
- the three migration fixture HOMEs;
- `--demo`, `--selftest` and `--snapshot` (existing) working on the signed build;
- a debug-only updater endpoint override.

## Risks and open questions

### Risks

- **R1 Redaction drift.** Regex semantics, Unicode classes, NFKC versions. Mitigation:
  vectors plus the differential (D2); `regex`'s linear time also removes a ReDoS
  exposure the JS version has.
- **R2 Installer fidelity.**
  - JS quirks: integer-like key reordering, string re-escaping, lone surrogates,
    duplicate keys.
  - Using serde_json's `arbitrary_precision` would leak through feature unification
    into Tauri.

  Mitigation: the separate lossless module, the differential, and DIVERGENCES.md. The
  Rust version keeps original key order where Node didn't, which only makes round
  trips more exact.
- **R3 `.env` parsing.** Reduced to a read-only legacy import using the existing,
  vector-tested Rust parser.
- **R4 A brain gap during updates and migration.** Under 1 s, but a hook in that
  window shows one "hook error" notice [1]. Accepted unless Q11 says otherwise. The
  fallback is a socket hand-off, which costs complexity.
- **R5 The WebKit SPI goes away.** The strip could be throttled. It fails soft
  (guarded), is logged, and doctor warns. There's no known public replacement. If it
  breaks, the fallback is to look at a different window configuration.
- **R6 SMAppService agent pointing at the main executable.** It may behave unexpectedly
  (for example LaunchServices treating it as an app launch, or Login Items wording).
  M1 proves it. The fallback is a second executable in the same bundle and version
  (D1).
- **R7 TCC attribution.** CLI-raised prompts would name Terminal; hence the action
  hand-off (D4).
- **R8 Identity changes.** Changing the bundle id or Team ID resets every user's
  grants [17]. A Developer ID certificate's expiry doesn't change the Team ID, so
  renewal is safe. A revoked certificate stops new launches of signed builds until
  re-signed.
- **R9 Losing the updater key.** Existing installs could never be updated again; keep
  an offline backup. Losing the notary key is recoverable (revoke, reissue).
- **R10 Art delays or quality.** It runs in parallel from M1. The release can't ship
  1.0 without it, but rc builds can use a placeholder pet and import.
- **R11 Homebrew official-repo acceptance.** Notability [23]; the project's own tap
  first.
- **R12 Claude Code changes.**
  - HTTP hook support per event can change: SessionStart and Setup are already
    excluded [1].
  - New fields like `onFailure` [1] show the schema moves.

  doctor's `--e2e` check and the recorded-event fixtures are the tripwire; run `doctor
  --e2e` in the release checklist.
- **R13 Universal binary size and Intel support.** Q3.
- **R14 Brain-mode footprint.** Linking Tauri in a background process could cost more
  memory than expected. M1 measures it; the fallback is D1's second executable.
- **R15 Running two brains.** A legacy LaunchAgent brain and the app's brain fight
  over 47417. The startup check, doctor and the migration's port check cover it.

**Assumptions** (to confirm):

- A1. The universal build is wanted.
- A2. A single `v*`-tag release flow is acceptable.
- A3. The Automation entitlement works with osascript as a child (M1 proves it).
- A4. No JIT entitlement is needed (M1 proves it).
- A5. Onboarding recommends start at login and warns, rather than refusing, if it's
  off.
- A6. Repository dev builds keep `local.bellwether` as their identifier.
- A7. Player tests stay in JavaScript permanently.

### Open questions for the maintainer

1. **Bundle id.** Which reverse-DNS id (from a domain or account the maintainer
   controls) becomes permanent for the signed app? It can never change after the first
   public signed build.
2. **Dev identity.** Keep `local.bellwether` for repository dev builds (recommended,
   so dev and installed builds never share grants or login items), or something else?
3. **Architectures.** Universal (Apple silicon and Intel), or Apple silicon only?
   macOS 26 is the last release for Intel Macs, so universal mostly serves older
   machines and roughly doubles the download.
4. **Art.** What budget is there for commissioning, and is CC-BY-4.0 (recommended) or
   MIT the licence for the sheet? Should the artist be credited by name in NOTICE? Is
   runtime-scaled lambs (0.6) enough, or should the commission include dedicated lamb
   frames?
5. **Homebrew.** Own tap only for now, or submit to the official cask repository once
   notability allows?
6. **`pets fetch`.** Keep it in the shipped CLI as an opt-in for the eSheep pet
   (pinned and sha256-checked, using the updater's TLS stack), or drop it in favour of
   `pets import` only? Recommended: keep it as a CLI-only, opt-in command, never
   offered by onboarding.
7. **Update policy.** Check daily and prompt before installing (recommended), or
   download in the background and install on quit, or check only on request?
8. **Notarization credentials.** App Store Connect API key (recommended) or Apple ID
   with an app-specific password?
9. **Minimum macOS.** Is 13.0 still right? SMAppService needs it; raising it to 14 or
   15 would trim test matrix work.
10. **Node brain deletion.** At 1.0 (after one rc cycle), or one full release after
    1.0?
11. **Update gap.** Is a sub-second window in which one hook may show an error
    acceptable during updates and migration, or is a zero-gap socket hand-off worth
    the extra complexity?
12. **Start at login off.** If the user turns start at login off, should onboarding
    still allow installing hooks (with a warning, recommended), or refuse until it's
    on?
13. **CLI install for DMG users.** Is `~/.local/bin` acceptable, or should there also
    be an admin-prompted `/usr/local/bin` option?
14. **Legacy headers.** The rename commit accepts `X-Pasture-*` for "one release". Does
    that release count as the last repository release or the first app release? This
    decides whether the Rust brain carries the legacy acceptance at all.

---

### Decisions (maintainer, 2026-10-11)

The maintainer accepted these answers to the open questions above:

1. Bundle id for the signed app: `com.fanzootechnology.bellwether`. Permanent from the first signed build.
2. Repository dev builds keep `local.bellwether`.
3. Universal build (Apple silicon and Intel).
4. Art: deferred until this version is ready for distribution; the maintainer has a concept for an original pack. Lamb frames and the licence are decided with it.
5. Homebrew: own tap first.
6. `pets fetch` for eSheep stays as an opt-in CLI command, clearly labelled.
7. Updates: check daily and ask.
8. Notarization with an App Store Connect API key.
9. Minimum macOS 13.0.
10. The Node brain is deleted at 1.0.
11. A hook error during the brain's sub-second restart on updates and migration is acceptable.
12. Without start at login, onboarding still allows installing hooks, with a clear warning.
13. The CLI goes in `~/.local/bin` for DMG users; Homebrew users get it on their PATH.
14. Accepting `X-Pasture-*` headers ends with the last repository release, so the Rust brain does not implement it.

## Sources

1. Claude Code, Hooks reference (HTTP hook fields, response handling, timeouts in
   seconds, `onFailure`, SessionStart restrictions, allowlists, the settings file
   watcher): https://code.claude.com/docs/en/hooks
2. Tauri 2, macOS code signing (signing identity, certificate import, notarization
   environment variables): https://v2.tauri.app/distribute/sign/macos/
3. Tauri 2, macOS application bundle (entitlements, `files`, merged Info.plist, usage
   descriptions): https://v2.tauri.app/distribute/macos-application-bundle/
4. Tauri 2, Updater plugin (signing keys, `latest.json`, platform keys, Rust-only use,
   Rust ≥ 1.90): https://v2.tauri.app/plugin/updater/
5. Tauri 2, Embedding external binaries: https://v2.tauri.app/develop/sidecar/
6. Tauri 2, Autostart plugin (LaunchAgent launcher): https://v2.tauri.app/plugin/autostart/
7. auto-launch 0.6.0 docs (LaunchAgent and AppleScript modes only):
   https://docs.rs/auto-launch/0.6.0/auto_launch/
8. Apple, SMAppService (macOS 13+; replaces plists in ~/Library/LaunchAgents):
   https://developer.apple.com/documentation/servicemanagement/smappservice
9. Apple, `SMAppService.agent(plistName:)` (plist must be in
   `Contents/Library/LaunchAgents`):
   https://developer.apple.com/documentation/servicemanagement/smappservice/agent(plistname:)
10. Apple, Updating your app package installer to use the new Service Management API
    (plists in the signed bundle; Login Items listing):
    https://developer.apple.com/documentation/servicemanagement/updating-your-app-package-installer-to-use-the-new-service-management-api
11. objc2-service-management 0.3.2, SMAppService bindings (register, unregister,
    status, openSystemSettingsLoginItems, statusForLegacyURL):
    https://docs.rs/objc2-service-management/0.3.2/objc2_service_management/struct.SMAppService.html
12. Apple, Notarizing macOS software before distribution ("not App Review";
    requirements: Developer ID, hardened runtime, secure timestamp, no
    get-task-allow; DMG supported):
    https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution
13. Apple, Resolving common notarization issues (`codesign --deep --strict`, `spctl`):
    https://developer.apple.com/documentation/security/resolving-common-notarization-issues
14. Apple, Customizing the notarization workflow (notarytool, containers):
    https://developer.apple.com/documentation/security/customizing-the-notarization-workflow
15. Apple, Hardened Runtime: https://developer.apple.com/documentation/security/hardened-runtime
16. Apple, Apple Events entitlement:
    https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.automation.apple-events
17. Apple Developer Forums, losing a TCC permission after updating an ad-hoc-signed app
    (designated requirement): https://developer.apple.com/forums/thread/795739
18. Apple Developer Forums, how TCC relies on the bundle ID:
    https://developer.apple.com/forums/thread/698337
19. tauri-apps/tauri-action (v1.0.0, `latest.json` upload):
    https://github.com/tauri-apps/tauri-action. See also
    https://v2.tauri.app/distribute/pipelines/github/
20. GitHub Docs, Secure use reference for Actions:
    https://docs.github.com/en/actions/reference/security/secure-use
21. GitHub Docs, Using environments for deployment:
    https://docs.github.com/en/actions/deployment/targeting-different-environments/using-environments-for-deployment
22. Homebrew, Cask Cookbook (`binary`, `auto_updates`, `uninstall quit:`, `zap`):
    https://docs.brew.sh/Cask-Cookbook
23. Homebrew, Acceptable Casks (notability, Gatekeeper):
    https://docs.brew.sh/Acceptable-Casks
24. Workbrew, Homebrew 5.0.0 (casks failing Gatekeeper lose support from
    2026-09-01): https://workbrew.com/blog/homebrew-5-0-0
25. regex crate docs (no look-around or backreferences; Unicode-aware classes by
    default): https://docs.rs/regex/latest/regex/
26. Authors Alliance, summary of the US Copyright Office's January 2025 report on AI
    and copyrightability:
    https://www.authorsalliance.org/2025/02/20/copyrightability-and-artificial-intelligence-a-new-report-from-the-u-s-copyright-office/
27. crates.io API, current versions queried 2026-10-10 (tauri 2.12.2,
    tauri-plugin-updater 2.13.2, tokio 1.53.2, hyper 1.12.0, objc2 0.6.5,
    objc2-service-management 0.3.2, regex 1.13.1, unicode-normalization 0.1.25,
    serde_json 1.0.151, ureq 3.4.2, tauri-plugin-dialog 2.8.1): https://crates.io
28. Tauri configuration reference (`bundle.macOS.hardenedRuntime` defaults to true,
    `mainBinaryName`, `bundle.resources`, `createUpdaterArtifacts`):
    https://v2.tauri.app/reference/config/

## Deviations

Recorded by the developer while building M1 (2026-10-12). Each one: what the plan said,
what is true, and what was done.

1. **No move to `app/`, no root workspace yet.**
   - Plan: the shell moves to `app/` in a workspace with `crates/core` and
     `crates/brain`.
   - True: the maintainer's live strip runs from
     `shell/src-tauri/target/release/bellwether-shell`, and the Node CLI, the tests,
     THIRD-PARTY and `.cargo/config.toml` all name `shell/src-tauri`. Moving the
     directory moves that binary (and its Accessibility grant).
   - Done: `shell/src-tauri` is the workspace root, and `crates/brain` is a member
     that points back with `package.workspace`. There is one `Cargo.lock`, in the same
     place. The move to `app/` waits until the live install has migrated off the
     repository (M8). `crates/core` comes with M3.
   - So plain `cargo test` from the repository root finds no workspace. Use `npm run
     test:cargo`, or run cargo in `shell/src-tauri`.
2. **The permanent id lives in an overlay, not in `tauri.conf.json`.**
   - Plan: the id goes in `tauri.conf.json`.
   - True: decision 2 keeps `local.bellwether` for dev builds, and a test pins it as
     the identifier there.
   - Done: `shell/src-tauri/tauri.dist.conf.json` (merged by `tauri build --config`)
     holds the permanent id, `mainBinaryName`, the entitlements, `infoPlist` and the
     agent plist. The merged Info.plist is `macos/Info.dist.plist` and is used only by
     that overlay, so dev bundles are unchanged.
3. **No legacy `X-Pasture-*` acceptance in the Rust brain.** M1 listed it; decision 14
   ends it with the last repository release. A test pins that `X-Pasture-Token` is
   refused.
4. **Local notarization uses a notarytool keychain profile.**
   - Plan: the bundler's `APPLE_API_*` environment variables.
   - Done: `scripts/build-app.mjs` signs through the bundler, with the identity's
     SHA-1 so no name carrying the Team ID is passed around. It then runs `xcrun
     notarytool submit --keychain-profile bellwether --wait`, `stapler staple`,
     `stapler validate` and `spctl`.
   - CI (M9) can still use the API-key variables.
   - `--adhoc` was added for local proving without the keychain.
5. **A `/hook` body that doesn't arrive within 30 s.** Node's `requestTimeout` answers
   408. A `HookAnswer` has only 204 and 403, so Rust logs it and answers an uncounted
   204. This goes on the DIVERGENCES list in M2.
6. **`brain-server.test.mjs` guard cases go to M3.** They test `/state` and
   `/control`, which don't exist until M3. The `/hook` matrix is covered by the
   black-box suite.
7. **The Automation probe sends `count windows`, not `version`.** `version` sends no
   Apple Event (F2), so D6 step 5 must not use it.
8. **Logs with `BELLWETHER_HOME` set.** The brain writes `<home>/logs/brain.log`
   instead of `~/Library/Logs/Bellwether/`, so a test home keeps everything inside it.
9. **The local build is arm64 only.** Decision 3 says universal, but only
   `aarch64-apple-darwin` is installed here. `--target universal-apple-darwin` needs
   `rustup target add x86_64-apple-darwin`. Not done here; left for M9 and CI.
10. **`/health` key order.** serde_json writes keys sorted. Node writes them in source
    order. No consumer depends on the order, and the tests compare key sets.
11. **THIRD-PARTY.** The four new crates (`httpdate`, `objc2-service-management`,
    `objc2-security`, plus the path crate `bellwether-brain`) are listed. The
    `@tauri-apps/cli` row used to depend on whether `shell/node_modules` was installed,
    and installing the Tauri CLI to build the app made `--check` fail. Its licence is
    now pinned in `scripts/third-party.mjs`, so both states render the same row.
12. **`SMAppService` `notFound`** is also how macOS reports a service that was never
    registered (F1). Doctor shows it as INFO with that meaning, not as a broken bundle.
13. **Ad-hoc builds get their own id.** The plan has one id for every signed build.
    F7 showed that a login item registered from an ad-hoc build under the permanent id
    breaks the Developer ID build's. So `build-app.mjs --adhoc` uses
    `com.fanzootechnology.bellwether.adhoc`, and `autostart install` refuses an ad-hoc
    bundle that carries the permanent id.
14. **`autostart` in an old login session** (F9) exits 2 with an explanation instead of
    calling SMAppService. Doctor reports login items and Accessibility as unknown there.
15. **Test builds can't register the app login item** (F10), and `autostart uninstall`
    stops this bundle's running strip (F11). Neither is in D5; both are needed for
    M6.

Recorded by the developer while building M2 (2026-10-13).

16. **The cross-target check covers `bellwether-brain` only.**
    - Plan: `cargo check` of core and brain for Linux and Windows.
    - True: `crates/core` doesn't exist until M3 (deviation 1).
    - Done: CI checks `-p bellwether-brain --all-targets` for both targets. The workflow
      says where core joins the list.
17. **The `__` commands: one works, the rest are stubs, and two were added.**
    - Plan: `__fixture`, `__redact`, `__normalise`, `__json`, `__diff`.
    - True: none of those has a Rust implementation yet. The `.env` reader is the only
      ported area, and D2 lists `.env` lines for the differential.
    - Done: `__env` works. `__list` tells the differential which areas are ported. The
      five planned commands answer "not ported yet (milestone N)" with exit 1, until
      their milestone. Every command takes cases as JSON lines on stdin and answers one
      JSON line each, so 10⁵ cases go through one process.
18. **The release check is `cargo build --release`, not a full `tauri build`.**
    - Plan: a release `tauri build` contains no `__` commands.
    - True: `tauri build` compiles with `cargo build --release` and the Tauri configs'
      `build.features`, then bundles and signs. Running it here would replace M1's signed,
      notarized bundle in `target-dev/release/bundle`. CI can't sign.
    - Done: locally, `cargo build --release --target-dir
      shell/src-tauri/target-dev/release-check`; in CI, `cargo build --release`. Then
      `__fixture` must exit 1 with `unknown command "__fixture"`. A cargo test
      (`release_builds_never_enable_the_conformance_feature`) checks that no default
      feature, no Tauri config's `build.features` and nothing in
      `scripts/build-app.mjs` turns the feature on.
19. **What a parity-table row is.**
    - Plan: CI publishes test file, node result, rust result.
    - Done: the rows are the files `test/conformance.json` assigns to a milestone, plus
      every other file that starts the brain through the helper. A file that only tests
      `brain/*.mjs` in process has nothing to run against Rust, so its rust cell says
      "not wired" (red), not a vacuous pass. A rust pass for a file that also tests JS in
      process is marked `pass *`. The run fails when a node cell is red, or when a row
      the current milestone requires (`"current": "M2"`, so only M1's
      hook-route-blackbox) is red under rust.
20. **`npm test` keeps its count by reading the vectors inside existing tests.** No Node
    test was added or removed; the tests the cases came from loop over the vectors and
    assert the Node answer as well as their original checks. `tester-m1`'s fixture
    privacy scan now walks subdirectories, so `installer-vectors/` gets the same scan.
21. **`Options::request_timeout` in the brain crate.** Pinning deviation 5 at the real
    30 s would make a slow test, so `Options` (now built with `Options::new`) carries the
    body timeout, `REQUEST_TIMEOUT` by default. Only tests set it shorter.
22. **Under rust the helper refuses `autostart`.** The Rust `autostart` calls SMAppService
    for real, and the Node tests that call it expect a LaunchAgent plist in a temp HOME.
    Those tests are red under rust until M6 ports the command and tests it on a spare
    account.
23. **The differential's `.env` oracle sample.** Besides comparing Node with Rust, it
    runs 200 generated `.env` files through `node --env-file` itself. Two known shapes
    are reported, not failed (M2 findings, DIVERGENCES.md); any other difference fails.

Recorded by the developer while building M3 (2026-10-13).

24. **`crates/core` uses more than std and serde.**
    - Plan (Cargo layout): core "uses only std and serde".
    - True: redaction needs `regex` and `unicode-normalization`, the WebFetch summary needs
      the WHATWG host (`url`), the fingerprint and sheep ids need `sha2`, the token needs
      `getrandom`, and the flock's ordered maps are `indexmap`. All but
      `unicode-normalization` were already in the lockfile; the Stack table lists most.
    - Done: those six, and no serde: core's JSON is its own (deviation 25).
25. **JS strings and JS JSON in Rust.**
    - Plan: serde_json for snapshots and IPC; a hand-written lossless JSON for the
      installer only (M4).
    - True: `/state`, recordings, state.json and the fixture lines are `JSON.stringify`
      output, and the bubble text inside them is bounded and cut in UTF-16 code units.
      serde_json sorts no keys the JS way, formats numbers differently and refuses lone
      surrogates.
    - Done: `jsstr.rs` keeps every JS string as code units (one `char` each, a surrogate
      mapped into U+F0000–U+F07FF), and `json.rs` is `JSON.parse` / `JSON.stringify` over
      JS values (f64 numbers, JS key order, a 1000-level depth limit). The M4 lossless
      module is still to come. serde_json stays for `/health` and the strip. So lone
      surrogates turned out not to be a divergence.
26. **The test bridge: `__session` and `test/impl.mjs`.**
    - Plan (M3 brief): wire the in-process files through the `__` commands or the CLI.
    - True: those files call stateful objects (a Flock, a publisher) synchronously, call
      by call; flock.test's identity cap alone makes about 6000 calls. A process per call
      is too slow, and the tests read each answer in line.
    - Done: `__session --in <fifo> --out <fifo>` (conformance builds only) keeps objects by
      handle and answers one JSON line per call; `test/impl.mjs` has the brain/*.mjs names,
      sends each call and blocks on the answer FIFO. `startBrain` is a real `serve` (or
      `record`) process; `brain.flock` reads `/state` and state.json. `test/conformance.mjs`
      counts a file that imports `test/impl.mjs` as wired.
27. **Tests with nothing in the Rust brain yet are skipped under rust, with the reason.**
    - Plan: the files M3 names pass under rust.
    - True: several of them also test M4 commands (doctor, hooks) or Node-only parts (the
      `/bin/sh` shim, the `.env` writer).
    - Done: `notUnderRust(reason)` (test/helpers.mjs) skips such a test under rust only; the
      parity table shows it as skipped, never as passed. Each is listed in the M3 status.
      M4 removes its ones.
28. **The harness's Rust environment.**
    - M2: the binary got a temp home whose config.json held only the test port.
    - Done: the home is the Node CLI's `BELLWETHER_DATA_DIR` (so state.json,
      shell-state.json and recordings land where the tests look), and the whole config file
      passes as `BELLWETHER_CONFIG` (the Rust CLI reads it, as the Configuration table says).
      A config with no port or 47417 is copied with the suite's port; the live-port refusal
      is kept.
29. **Test knobs in `serve`.** `BELLWETHER_TEST_TICK_MS`, `_PERSIST_MS`, `_PUBLISH_MS`,
    `_MAX_CONNECTIONS` and `_NO_FD_RAISE` are read only in conformance builds (a `conformance`
    feature of bellwether-brain that only the shell's turns on; a cargo test checks neither
    is a default). They stand in for brain.mjs's `tickMs`, `persistMs` and `publishMs`
    options. `serve` and `record` also take `--port` (0 means any free port).
30. **Dev builds optimise the hot crates.** `profile.dev.package` gives regex,
    unicode-normalization, sha2, tokio, hyper and their helpers opt-level 3 (debug
    assertions off for the regex and server crates), and core and brain opt-level 2. A
    plain debug build missed the 5 MB redaction budgets (up to 34 s) and the 5 ms p99.
    Also: state.json is written without fsync (as Node writes it; it is rewritten every few
    seconds on the brain's only thread), the token file with it; the built-in patterns
    compile when the cleaner is made, not on the first hook.
31. **`token rotate` does steps 1 and 3 of D3; step 2 waits for the installer.**
    - Plan: write the token file, rewrite the hook entries, tell the brain.
    - True: rewriting hook entries is the installer (M4).
    - Done: `token rotate` is a dry run; `--write` writes a new token file and posts
      `reload-token` with the old token (the brain accepts it for a minute more). When the
      settings file (`--settings`, default `~/.claude/settings.json`) carries the current
      token, `--write` refuses (exit 3, nothing changed), since those hooks would fail. M4
      adds the rewrite. `token init --from <.env>` imports a repository install's token (the
      one use D3 leaves the `.env` reader).
32. **Both bounds on connections.** The M1 review asked for a cap that closes idle
    connections first, or a short idle timeout. Done: both, plus raising the descriptor
    limit: 5 s idle timeout; a cap of 1024 (fd limit minus 64 when lower); eviction of the
    connection idle longest, else the oldest `/state` long poll (DIVERGENCES 11).

Recorded by the developer while building M4 (2026-10-14).

33. **Where doctor lives.**
    - Plan: doctor with the new checks; M1's doctor was in the shell crate's `cli.rs`.
    - Done: the checks that need no app bundle are in the brain crate, `crates/brain/src/
      doctor.rs` (config, token and its file's mode, `claude`, the brain and its token, the
      settings file, the other settings levels, palette, sharing names, the hidden strip, the
      strip's log: locator, Dock and the WebKit switch, pets, legacy LaunchAgents, `next`,
      `--e2e`), with the settings checks themselves in `crates/core/src/settings_checks.rs`.
      The shell's `cli.rs` adds what needs the bundle (identity, signature, app data, login
      items, Accessibility, the agent-port hint of review D3, the stale-session wording of
      F9) around them, and prints the Node doctor's format, `--json` included. The sharing
      rules stay the strip's (`privacy.rs`); the shell passes their answer in.
    - The `onFailure: "block"` FAIL went into the Node doctor too, so one test runs against
      both brains.
34. **"Every installer vector round-trips": the nine that can.**
    - Plan: for every installer vector, install then uninstall returns the original bytes,
      and previews are byte-identical to Node's.
    - True: two of the eleven vectors don't round-trip under Node either, by their nature.
      "Not in bellwether's layout" is reformatted by any install (its preview says so). "Exotic
      numbers, escapes and integer-like keys" has escapes such as `\/` and `é`, which
      R2's rule (escape as `JSON.stringify` does) rewrites as `/` and `é`; it also has the
      integer-like keys R2 keeps in file order, where Node moves them first.
    - Done: the nine vectors Node round-trips round-trip under Rust; ten of eleven previews
      and installed files are Node's bytes; the exotic one differs only in the order of its
      three integer-like keys, and after install and uninstall only its escaped string's line
      differs from the original (test/DIVERGENCES.md 16; `crates/core/tests/installer.rs`).
      Keeping escapes verbatim would have made that vector round-trip, at the price of a
      second difference from Node's preview; R2 asks for `JSON.stringify`'s escaping, so it
      wasn't done.
35. **`migrate`: the repository's steps into the app's files; no LaunchAgent swap.**
    - Plan: port today's repository-to-repository behaviour.
    - True: today's step 4 writes `local.bellwether.*` LaunchAgents that run Node from the
      checkout and boots the old ones out. The app never writes or boots out a LaunchAgent
      (D5), and its own swap (SMAppService, rollback) is milestone 8.
    - Done: steps 1–3 and 5 in today's order and wording, into `BELLWETHER_HOME`; step 4
      finds the old agents by what they run (pasture's and the repository's
      `local.bellwether.*`) and lists them, untouched. When HOME is the real home, the hook
      entries move only once a brain on the port takes the token under `X-Bellwether-Token`
      (a doctor ping in place of brain/probe.mjs); under any other HOME there is no ping and
      they move directly, as today's CLI does under a temp HOME. (Corrected after the M4
      review: this said the reverse.) Also from D9: the token
      may come from the old `.env`'s `BELLWETHER_HOOK_TOKEN` as well as `PASTURE_HOOK_TOKEN`,
      else from the hook entries; without `--from`, the old install is the WorkingDirectory
      of the old brain's plist; obsolete config keys are dropped from the copy, and the
      preview says which (test/DIVERGENCES.md 17).
36. **`token rotate` tells the brain before rewriting the entries.**
    - Plan (D3): write the token file, rewrite the hook entries, signal the brain.
    - True: in that order the entries carry the new token for a moment while the brain
      still accepts only the old one, and those hooks get a 403.
    - Done: the settings file is planned first (one that can't be rewritten stops the
      rotation with nothing changed), then the token file, then `reload-token` (the brain
      accepts both for a minute), then the entries. If the write fails, the old token goes
      back into the file and the brain is told again. The `--write` refusal of deviation 31
      is gone.
    - After the M4 review (D1): the entries change only when the brain confirmed the switch
      or no brain answers. A brain that answers and refuses (the Node brain answers
      `reload-token` with 400) gets the old token back in the file and the entries
      untouched, exit 1. The second `reload-token` after a failed write is checked, and the
      message names whichever of brain, file and entries is off.
37. **`pets import`: what "tiles at most 64×64" means, and where the pins are.**
    - Plan (D8): at most 16 MiB, UTF-8, no DOCTYPE or ENTITY, it parses, tiles at most
      64×64; copy to `pets/<name>/`, pin in `pets.user.lock.json`, propose an
      `animationMap`.
    - Done: `tilesx` and `tilesy` from 1 to 64 (the player's limits, `validatePet`); the PNG
      is checked by its signature, not decoded. The pins are `<home>/pets.user.lock.json`.
      The name is `--name`, else the file's folder for an `animations.xml`, else its stem.
      The proposed map is printed for config.json (which isn't edited). XML is read by a
      small well-formedness reader in `crates/brain/src/pets.rs` (no new crate). `pets fetch`
      answers that the app doesn't redistribute the art (Q6 keeps it a repository opt-in).
38. **`shell start|stop|restart`: the arguments only.** The plan keeps `shell …` in the
    CLI. Starting and stopping the app's strip needs the strip on the Rust brain and the
    app-data layout (M5), so in M4 the Rust CLI checks the arguments as Node does (exit 3
    for a bad action, `--dev`, `--release`) and says the actions come with M5 (exit 1).
39. **The test harness's HOME and the real settings file.**
    - Asked: under rust, force a temp HOME, and a guard test that fails if a test process
      could resolve the real `~/.claude/settings.json`.
    - Done: `rustEnv` (test/helpers.mjs) replaces a missing or real HOME with a temp one per
      test process; the preload (test/setup.mjs) refuses any read or write of the real
      home's `.claude/settings.json` and `settings.local.json` (the home from the password
      database, so a test's HOME can't hide it) in every Node test process. The guard test
      is in isolation.test.mjs; it only ever tries reads.
    - That guard tripped one existing test: tester-regression hashed the real settings file
      to prove it unchanged. It now compares size, mtime, ctime and inode (a write, a chmod
      or an atomic replace changes one of them) and never reads the file.
    - The helper's live-port refusal no longer applies to `hooks … --port`, which only names
      entries in a file (a test hands it `--port=047417` to check that it is refused).
40. **`runDoctor` under rust is a CLI run.** Its options become the binary's environment;
    a `runClaude` is answered through a fake `claude` on PATH that asks the test process
    over loopback, so the `--e2e` tests drive the Rust doctor's real `claude -p` path. A
    config port below 1024 (the tests' "nothing listens on port 1") becomes a released free
    port, since the Rust config refuses such ports; a palette `withDefaults` upgraded is
    written back as the old one, so the Rust doctor finds the upgrade itself.
41. **What stays skipped under rust, and why.** Each with `notUnderRust(reason)`:
    - the repository's LaunchAgent `autostart` (M6; the harness never runs the app's on this
      Mac, deviation 22) and `shell start|stop|restart` (M5);
    - the repository migration in process (brain/migrate.mjs with launchd injected,
      brain/probe.mjs; the app's is run through the CLI, deviation 35);
    - the `.env` writer and reader, `PASTURE_*` names, the shims, `pets fetch`, cargo and
      the repository's first-run walk and README (retired with the Node CLI, or the
      repository's own; test/DIVERGENCES.md 6, 15, 18);
    - `X-Pasture-*` acceptance by the brain (DIVERGENCES 4), Node's `parseArgs` and
      `nodeSupported` (their Rust counterparts are cargo-tested), and the plan-then-apply
      race test (in process; `crates/core` hooks.rs tests the Rust check).
    - New tests that run against both brains, each asserting its own brain's contract where
      the two differ on purpose: every installer vector through the CLI (11), the
      `onFailure` FAIL, `pets import`, the migrate CLI end to end and its error paths, the
      migrate previews of hub-labelled agents (3), `PASTURE_HOOK_TOKEN` alone, the hook
      commands in `help`, and the real-settings guard.
42. **conformance.json.** `current` is M4. Besides the files M4 names, isolation, envload,
    rename and pets are M4 rows: they were red or unwired under rust, now each runs the
    Rust binary and says why its Node-only tests skip.
43. **The differential's `json` area** generates more of R2's shapes than M2's generator:
    lone surrogates and control escapes, key escapes, `"01"` and `"4294967295"` (not
    array indexes), indents of 12 characters (cut to 10, as `JSON.stringify` cuts the gap)
    and of tabs, inline and multi-line arrays, stray line endings, leading blank lines,
    one-line objects and a top-level array. A difference counts as explained only when the
    case has an integer-like key and Node's own reading of Rust's answer gives Node's
    answer; two refusals count as the same answer (DIVERGENCES 21).
44. **`EVENTS` moved to crates/core** (`hooks.rs`); the brain's hook route re-exports it,
    so the installer and the brain can't disagree on the 16 events.

Recorded by the developer while building M5 (2026-10-14).

45. **The shell keeps a repository layout until M8.**
    - Plan: the shell reads `hook-token` and config from Application Support and logs to
      `~/Library/Logs/Bellwether`.
    - True: the maintainer's live install runs `target/release/bellwether-shell
      --no-spawn-brain` from a LaunchAgent, next to the Node brain, which reads the checkout's
      `.env`. If the shell read only the app's files, the next `bellwether shell build
      --release` would leave his strip without a token (or with a different one), and his sheep
      would stop before M8's migration exists.
    - Options: (a) a repository layout chosen by where the binary is; (b) a written procedure
      for repository users (`token init --from .env` and a config copy before rebuilding).
      (b) splits one install's settings across two places and depends on the user doing it
      before a rebuild; (a) needs nothing from them.
    - Done: (a). Inside an app bundle the shell uses the app's files; any binary outside one
      keeps the M4 behaviour (the checkout's `config.json`, the `.env`/environment token,
      `data/`, `data/shell.log`). `--no-spawn-brain` keeps working in both. The repository
      layout goes with the Node brain in M11; M8 moves the live install. The plan's M5 status
      has the hand switch-over.
46. **A test build's strip takes its bundled port and home.**
    - Plan: F10 recommended that the app read its port and home only from Application
      Support; M1 baked them into the agent plist only.
    - Done: the strip reads the same plist values (`login_items::bundle_agent_overrides`), so
      a test build's strip never uses 47417 or the real folder, however it is started (Finder,
      `open`, a login item). A `BELLWETHER_HOME` different from the bundle's is refused, exit 3.
      The `autostart` refusal of the app item for test builds stays.
47. **No bundled pet until M10.**
    - Plan: `pets.default` defaults to `bellwether`, the bundled original sheep.
    - True: the art is M10's. eSheep can't be bundled.
    - Done: the lookup is in place (bundled, pinned by the compiled-in `pets.lock.json`; then
      imported), the default name is `bellwether`, and until it exists the strip says, in
      `shell.log`, to `pets import` a pet and set `pets.default`. No `bundle.resources` entry is
      added yet. The repository layout keeps `esheep64` from `data/pets`.
48. **`shell start|stop|restart` in the app.** Not specified beyond "as fits the app"
    (deviation 38). Done: start opens this bundle through LaunchServices (`open -g -a`,
    passing `BELLWETHER_HOME`) unless its strip already runs, and waits up to 10 s for it;
    stop sends SIGTERM to this bundle's strip only (by `proc_pidpath`, the F11 rule) and waits
    5 s; restart is both. Outside a bundle they refuse with exit 2 and point at the repository
    CLI (DIVERGENCES 19).
49. **The version check: when, and which brain.**
    - Plan (D5): the app compares its version with `/health` and restarts the agent.
    - True: the Node brain also reports `0.1.0`, so the version can't tell a repository brain
      from the app's; only the pid can say whose a brain is.
    - Done: once, at the strip's start, before it polls `/state`. Only a brain whose pid runs
      this bundle's executable with ppid 1 and is the pid `launchctl print` gives for this
      bundle's agent label (this app's login agent; the launchd check added after the M5
      review) is restarted (`launchctl
      kickstart -k gui/<uid>/<bundle id>.brain`, 10 s limit); the strip then polls `/health`
      every 50 ms for up to 10 s and logs the gap. Another version that isn't this app's agent
      is left alone with a warning in `shell.log`. Measured: 76 ms (above). M9 measures it
      under the real updater.
50. **The brain child, in detail.** The child's stdout and stderr are discarded (the brain
    writes its own `brain.log`, so the old `brain-from-shell.log` is gone); it is stopped with
    SIGTERM, then SIGKILL after 2 s; a child that dies is restarted at most once every 10 s;
    the watchdog looks every 250 ms instead of every second, so "within a second" holds with
    room (97 ms measured). In the repository layout the child gets the checkout's `data/` as
    `BELLWETHER_HOME`, its `config.json` as `BELLWETHER_CONFIG` and the token in its
    environment: the Rust brain on the repository's files, since there is no Node spawn any
    more.
51. **`pets import` printed the proposed map with `?` for its line breaks** (M4: the whole
    pretty JSON went through `printable`). It is now printed line by line; JSON already
    escapes control characters inside strings. pets.test.mjs pins the layout.
52. **The single-instance lock is per port** (BR-3K's M5 observation 6).
    - Plan: one strip at a time (the plugin, keyed by the bundle id).
    - True: test bundles share the installed app's id and dev builds share the live
      repository strip's `local.bellwether`, so a test or dev strip handed over to the live one
      and quit (M5 status), and the by-hand checklist couldn't run a test strip beside the app.
    - Done: the runtime identifier the lock is named after is `<id>.p<port>` for any port but
      47417, unchanged on 47417 (so the lock matches every earlier build on the live port).
      Two strips on one port still exclude each other; strips on different ports coexist.

Recorded by the developer while building M6 (2026-10-15).

53. **`autostart settings`, a new subcommand.**
    - Plan (D5): on `requiresApproval`, `autostart status` and onboarding offer
      `openSystemSettingsLoginItems`.
    - True: a CLI can only offer by asking, and a script must never get a window it didn't ask
      for.
    - Done: after `install --write` or `status` finds an item waiting, the CLI asks "Open System
      Settings > General > Login Items & Extensions now? [Y/n]" when stdin and stdout are both a
      terminal, and otherwise prints the command that opens it: `bellwether autostart settings`
      (also `--via-app`, the action `login-items-settings`). M1 opened the pane unasked after a
      registration that needed approval; that is gone.
54. **Exit codes of `autostart`.**
    - `install --write` exits 2 ("not running") when an item ends `requiresApproval`: macOS
      won't start it until the user acts, so a script must not read it as done.
    - Outside Bellwether.app, `install` and `uninstall` exit 2 before any preview, saying
      nothing was registered or written. Before, the preview said "would register … (now
      unavailable)" with exit 0 and `--write` failed per item with exit 1. `status` still prints
      `unavailable` with exit 0.
55. **What counts as a legacy agent, and how launchd is asked.**
    - Plan (D5): the app shows a line "if any are still loaded".
    - True: a plist that isn't loaded (booted out by hand, or the user hasn't logged in since)
      comes back at the next login and fights the app's brain then; a loaded agent whose plist
      was deleted runs until logout. M4's doctor looked at plists only.
    - Done: an agent counts when its plist is present or launchd has it loaded, and the line
      says which (`crates/brain/src/legacy.rs`). launchd is asked by `launchctl print
      gui/<uid>/<label>` (read-only; four calls, about 3 ms each), for this user's GUI domain
      only (where a LaunchAgent loads at login); an answer counts only when its first line is
      exactly that target. Not asked: other users' domains, `user/<uid>` and `system` (a brain
      loaded there by hand would still hold the port; the migration's port check in M8 names
      whatever holds 47417). Only the four labels count: an agent under another label that
      runs a repository brain (the hub-labelled ones `migrate` finds by what they run) isn't
      named here. Asked only when HOME is the real home (the passwd entry), the
      same rule as `migrate`'s brain ping: under a temp HOME the real launchd state would be
      the wrong answer, and the test harness always uses a temp HOME.
56. **The harness runs `autostart` under rust (deviation 22, narrowed).** A development binary
    can't reach SMAppService (`login_items::service` refuses outside a bundle, before any
    call), so the harness allows it; it still refuses `autostart` for a `BELLWETHER_BIN`
    inside a `.app`, and `autostart settings` always (it opens System Settings).
57. **No spare macOS user account.** The test plan puts M6 on a spare account. There is none
    here, so: the temp-HOME test, unit tests for every status and refusal, and the brain item
    live from a signed test bundle with `--brain-only` on this account (F10 refuses the app item
    for test builds). The app item, its Login Items listing and the reboot are the
    maintainer's by-hand checks (M6 status).
58. **The tray's notices.**
    - Plan (D5): one line in the tray for legacy agents.
    - Done: that line, greyed (information only; the app never stops them), plus one line per
      login item waiting for approval, which opens the Login Items pane (D5's "offer" in the
      app). Found once at the app's start, as D5 says, in the app layout only: the repository's
      strip is itself started by a legacy agent, and `--demo` and `--selftest` are for checks.
59. **The "nothing written to LaunchAgents" test is a cargo test**
    (`shell/src-tauri/tests/no_launchagents.rs`), not a Node test: the Node CLI's `autostart`
    writes LaunchAgents by design, so the test has nothing to run against Node. It runs the
    binary's CLI and its `--action` side, not the strip (a window); the strip's only new code
    there is the startup check, which `legacy.rs`'s `scanning_only_reads` and tester-m3's
    tray.rs scan cover.
60. **`autostart install` refuses the brain agent without a token in its home** (BR-3K's M6
    review). The agent gets no environment and reads only `<its home>/hook-token`; without a
    usable one it exits 3 at once and launchd restarts it every 10 s, and nothing said so.
    - Choice: refuse rather than warn. A registered agent that can never run is worse than no
      agent, and the fix (`bellwether token init`) is one command the message names (with
      `BELLWETHER_HOME=<home>` for a test build).
    - The check is in `login_items::preflight` (registering the brain only; the app item and
      unregistering aren't affected), so the CLI, `--via-app` and the app's `--action` all get
      it. It reads the agent's home as the agent will: the bundle plist's BELLWETHER_HOME, else
      `~/Library/Application Support/Bellwether` under HOME, never the environment's token. A
      dry-run `install` says the same, since it runs the preflight too.
