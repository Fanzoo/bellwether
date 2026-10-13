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
