# Where the Rust brain differs from the Node brain, on purpose

The Node brain is the oracle until it is deleted (distribution plan D2). The same tests run
against both (`npm run test:rust`), and the differential (`npm run differential`) feeds both
the same generated inputs. Every difference they find is a bug, unless it is listed here
with the test that pins the Rust behaviour.

Add an entry when a port milestone makes a difference on purpose. Each entry says what Node
does, what Rust does and why, and names the test.

## In effect

### 1. A `/hook` body that stalls gets an uncounted, empty 204 (Node: 408)

- **Node:** `server.requestTimeout` (30 s) answers 408 when the body hasn't arrived.
- **Rust:** a `HookAnswer` can only be 204 or 403, so writing anything else does not
  compile (plan D1). The brain logs "dropped /hook: the body didn't arrive within 30 s",
  answers an empty 204, and changes no counter.
- **Why it's fine:** Claude Code gives up after its own `timeout: 1` long before 30 s, and a
  204 can't carry hook output. Since M1 (plan deviation 5).
- **Pinned by:** `crates/brain/tests/http.rs`, `a_stalled_body_gets_an_uncounted_empty_204`
  (a 300 ms request timeout, set through `Options::request_timeout`).

### 2. `/health` writes its keys sorted

- **Node:** keys in source order (`ok, name, version, pid, port, lastEventAt, mode,
  counters`).
- **Rust:** serde_json writes object keys sorted, at the top level and in `counters`.
- **Why it's fine:** nothing reads `/health` by key order. The CLI, doctor and the tests
  compare key sets. Since M1 (plan deviation 10).
- **Pinned by:** `crates/brain/tests/http.rs`, `health_keys_are_written_sorted`. The key
  sets of both brains are pinned by `test/hook-route-blackbox.test.mjs`, "/health needs
  no token and carries no content".

### 3. `/health` counters include `dropped` and `logDropped`

- **Node:** has neither counter. Its log lines go to stderr through Node's own buffering.
- **Rust:** `counters.dropped` counts events the state machine couldn't take in time: `/hook`
  hands each event to it with a `try_send` into a queue of 4096 (plan D1), and a full queue
  counts here instead of delaying the answer. `counters.logDropped` counts log lines dropped
  because a sink (stderr, brain.log) wasn't keeping up: each has a writer thread behind a
  queue of 1024, so a stderr pipe nobody reads stalls that thread, never `/hook` (BR-3K's M3
  finding: before, the brain froze at about 64 KB of log).
- **Pinned by:** `crates/brain/tests/http.rs`, `health_carries_no_content` and
  `health_keys_are_written_sorted`; `test/hook-route-blackbox.test.mjs`, "3000 hooks with a
  full stderr pipe" (both brains; `logDropped > 0` under rust; it hangs with a blocking
  logger).

### 4. `X-Pasture-*` headers are not accepted

- **Node:** for one release after the rename, a hook carrying `X-Pasture-Token` (and the
  `X-Pasture-*` terminal headers) is accepted (`test/rename.test.mjs`, "a running brain
  accepts a hook sent by an entry pasture wrote").
- **Rust:** refused with an empty 403, like any missing token. Maintainer decision 14 ends
  the legacy acceptance with the last repository release.
- **Pinned by:** `crates/brain/src/hook.rs`, `the_guards_in_order` ("the pre-rename
  header is not accepted"), and `crates/brain/tests/http.rs`,
  `hook_answers_are_status_only_and_the_guards_refuse`.

### 5. User `bubble.redactPatterns` are compiled by Rust's `regex`, so look-around and backreferences are refused

- **Node:** a pattern is a JS regular expression (no `u` flag; a leading `(?i)` is the `i`
  flag), so look-ahead, look-behind and backreferences work.
- **Rust:** the pattern is translated to `regex` syntax with the JS meanings kept (`\d`, `\w`,
  `\b` ASCII; `\s` JS's set; `.` without line terminators; `{` literal unless it starts a
  quantifier; `[]` and `[^]`; Annex B class ranges). `regex` has no look-around and no
  backreferences, and guarantees linear time, so a pattern using `(?=`, `(?!`, `(?<=`, `(?<!`,
  `\1`… or `\k<name>` is a config error, exit 3, naming the pattern and why (plan D2, R1).
  A pattern neither engine compiles is refused by both, but the reason in brackets is the
  regex crate's words, not V8's. Case-insensitive matching of non-ASCII letters follows
  Unicode simple case folding (`ß` matches `ẞ`), which JS without the `u` flag doesn't do.
- **Pinned by:** `crates/core/src/redact.rs`,
  `translation_keeps_js_meaning_and_refuses_what_regex_cannot_do`;
  `crates/core/tests/vectors.rs`, `config_answers_every_vector_as_node_does`; and
  `test/brain.test.mjs`, "the example config validates" (the reason text compared up to the
  bracket under rust). The differential's patterns (`\b`, classes, `{n,}`, `(?i)`, a literal
  `{`) agree on 10⁵ cases.

### 6. `token init` writes the token file; the `.env` writer is retired

- **Node:** `token init` adds `BELLWETHER_HOOK_TOKEN` to the repository's `.env` (fill in
  place, CRLF kept, symlinks followed, a `.bak`), and the CLI reads the token from there.
- **Rust:** `token init` writes `<home>/hook-token` (one line, 0600, in a 0700 directory), only
  if there is none; `--from <.env>` imports a repository install's token (read with the shared
  `.env` reader, below) so installed hooks keep matching. The app reads no `.env` (plan D3).
  `token rotate [--write]` is new.
- **Tests retired with the Node CLI** (skipped under rust with this reason):
  `test/token.test.mjs`, the three "bellwether token init (D4)" tests that write a `.env` and
  four of the "M10-8" tests; `test/tester-m1-reverify.test.mjs`, "an already-exported token wins
  over the .env"; since milestone 4 also `test/tester-m10.test.mjs`, "bellwether token init";
  `test/tester-regression.test.mjs`, the two first-run `.env` tests; `test/envload.test.mjs`,
  "which .env" and the two `BELLWETHER_ENV` tests (its "an exported token wins" runs against
  both); and the `/bin/sh` shims (`bellwether`, `pasture`).
- **Pinned by:** `crates/core/src/token.rs`, `init_writes_once_and_imports`;
  `crates/brain/tests/token_cli.rs`, `token_init_and_rotate`.

### 7. `/control` takes `reload-token`

- **Node:** an unknown action, 400.
- **Rust:** the brain reads its token file again and accepts the previous token for one more
  minute, so hooks in flight during a rotation don't fail (plan D3). A brain whose token came
  from `BELLWETHER_HOOK_TOKEN` answers 400 and says so.
- **Pinned by:** `crates/brain/tests/http.rs`, `reload_token_keeps_the_old_token_for_a_minute`.

### 8. A `config.json` without `port` means port 47417

- **Node:** `config.json` must set `port` (exit 3 otherwise).
- **Rust:** every key of the app's `config.json` is optional (plan, Configuration), `port`
  included. `validate_config` keeps the Node rule for the shared vectors; loading fills the
  default first. The test harness refuses such a config under rust (it would be the live port).
- **Pinned by:** `crates/core/src/config.rs`, `a_missing_port_is_the_default_in_the_app`.

### 9. Event and tool names that are members of `Object.prototype`

- **Node:** the reducer looks handlers and tool categories up on plain objects, so an event
  named `toString` or `constructor` (both pass the identifier check) creates a sheep that no
  handler moves, and `__proto__` makes `apply` throw after creating one (a consumer error); a
  tool named `constructor` gives a flavour that isn't a string.
- **Rust:** only the reducer's own event names have handlers, and only the listed tools have a
  category; anything else is ignored or `thinking`.
- **Why it's fine:** no Claude Code event or tool has these names; Node's behaviour is a JS
  quirk, not a contract.
- **Pinned by:** `crates/core/src/flock.rs`, `object_prototype_names_are_not_handlers`.

### 10. A hook body nested deeper than 1000 levels is malformed

- **Node:** `JSON.parse` takes any depth; `normalise` then fails somewhere past about 10 000
  levels (a stack overflow, counted as a consumer error).
- **Rust:** the parser stops at 1000 levels and counts the body as `malformed`, so nothing
  can overflow the brain's stack. Either way the answer is the empty 204 and no sheep moves.
- **Pinned by:** `crates/core/src/json.rs`, `parse_keeps_js_semantics`.

### 11. Connections are bounded

- **Node:** a keep-alive connection idle for 5 s is closed; there is no cap on open
  connections (Node raises its own descriptor limit).
- **Rust:** the brain raises its soft descriptor limit (to at most 8192); a connection with
  nothing in flight that has sent nothing for 5 s is closed, fresh or kept alive; and past
  1024 open connections (fewer if the limit is lower) a new one closes the connection idle
  longest, else the oldest `/state` long poll (whose client polls again). A hook in flight is
  never closed (milestone 3, from the M1 review).
- **Pinned by:** `test/hook-route-blackbox.test.mjs`, "300 idle connections open under a
  256-file limit" (both brains; it fails on a Rust brain started without the fix);
  `crates/brain/tests/http.rs`, `the_cap_closes_idle_connections_first` and
  `idle_connections_time_out`.

### 12. Messages that named the `.env`, and log lines

- **Node:** a 403 from `/state` says "the brain refused the token in .env (403)"; a missing
  token says to add one to the `.env`. Log lines are `bellwether: …` and `<clock>  <event>`.
- **Rust:** the same causes without the `.env` ("the brain refused the token (403)"; "make one
  with `bellwether token init`"); log lines are `<ms> bellwether: …`. Every test matches on the
  cause ("refused the token", "brain not running", "listening on"), never on the `.env` words.

### 13. The app's CLI surface: `--port`, `version`, and its help

- **Node:** `serve` and `record` take no `--port` ("unknown option --port", exit 3; the port is
  config.json's); there is no `version` command (unknown command, exit 1); `help` lists the
  repository CLI's commands.
- **Rust:** `serve` and `record` take `--port <n>` (1024–65535, or 0 for any free port), so
  a test brain or a login item can run on another port without a config file;
  `bellwether version` prints `bellwether <version> (rust)`; `help` is the app's own text,
  listing what the app has: since milestone 4 `hooks`, `doctor` (with `--port`), `pets
  import`, `migrate` and `shell start|stop|restart` too, with the Node CLI's flags for the
  hook commands.
- **Pinned by:** `crates/brain/src/cli.rs`, `ports`; `shell/src-tauri/src/cli.rs`,
  `help_lists_the_brain_commands` and `unknown_commands_exit_1_and_bad_flags_exit_3`
  (`version`); `test/hook-route-blackbox.test.mjs` starts every Rust brain with
  `serve --port`.

### 14. doctor: the app's lines

- **Node:** checks Node's version (`node`), cargo (`cargo`), the `PASTURE_*` names it read
  (`old names`), the repository's LaunchAgents (`autostart`, `old LaunchAgents`, `strip
  running`), the fetched pet art, and points `next` at the repository's first run (pet art,
  a strip build, LaunchAgents). Entries pasture wrote are a WARN (`hooks from pasture`): the
  Node brain still reads `X-Pasture-*` for one release.
- **Rust:** none of those. Instead: the token file's mode (`token`, a FAIL when others can
  read it), keys the app no longer reads (`config keys`, WARN: `brain.nodePath`,
  `autostart`), LaunchAgents a repository install left behind (`legacy agents`, WARN: the
  four labels of plan D5), the strip's WebKit occlusion switch from its log (`webkit`), an
  imported pet against `pets.user.lock.json` (`pets`), and the shell's own lines: `identity`,
  `signature`, `app data`, `login: brain`, `login: app`, `accessibility` (with milestone 1's
  stale-session wording, F9, and the agent's own port, review D3). `hooks from pasture` is a
  FAIL, exit 3, because this brain refuses `X-Pasture-*` (entry 4). `next` follows the app's
  order: config, token, a brain, anything failing, hooks. Messages that named the `.env` name
  the token in use instead (entry 12).
- **Both:** a bellwether entry with `"onFailure": "block"` FAILs (`hook onFailure`, exit 3)
  in the user settings file and at every other level doctor reads (project, local,
  managed), new in milestone 4 for the Node doctor too.
- **Pinned by:** `test/doctor.test.mjs` (the Node-only `node` floor is skipped under rust),
  `test/rename.test.mjs` ("status and doctor point them out", by each brain's name for the
  line), `test/token.test.mjs` (the `next` order through the Rust `next_step`), and
  `crates/core/src/settings_checks.rs`, `a_full_install_passes_and_on_failure_block_fails`.

### 15. `PASTURE_*` environment names are not read

- **Node:** for one release after the rename, an exported `PASTURE_HOOK_TOKEN` (and every
  other `PASTURE_*` name, and a `.env` that only has the old name) fills the `BELLWETHER_*`
  of the same name, and doctor says so.
- **Rust:** reads neither (plan, Configuration). `bellwether migrate` reads an old `.env`'s
  `PASTURE_HOOK_TOKEN` (or `BELLWETHER_HOOK_TOKEN`) to import a legacy token.
- **Pinned by:** `test/rename.test.mjs`, "an exported PASTURE_HOOK_TOKEN alone" (each brain
  its own answer).

### 16. Integer-like keys in settings files keep file order

- **Node:** `JSON.parse` lists integer-like keys (`"1"`, `"10"`) first, so an install
  rewrites `{"b": 1, "2": …, "1": …}` as `{"1": …, "2": …, "b": 1}`, in the preview and the
  file, and an uninstall doesn't give back the bytes.
- **Rust:** the lossless JSON keeps file order (plan D2, R2), so those keys stay where they
  were. Everything else is Node's: number literals verbatim, a duplicate key's first place
  with its last value, `__proto__` an ordinary key, and strings written as `JSON.stringify`
  writes them, so an escape such as `\/` or `\u00e9` in a settings file is rewritten as
  `/` or `é` by both brains (decided in milestone 4: the preview stays Node's, and only the
  key order differs).
- **Pinned by:** `crates/core/tests/installer.rs` (the exotic vector: the keys stay `b`,
  `2`, `1`, and after install and uninstall only the escaped string's line differs from the
  original); `test/hooks.test.mjs`, "every installer vector through the CLI"; the
  differential's `json` area counts such cases as explained only when reading Rust's
  answer the way Node reads a file gives Node's answer exactly.

### 17. `migrate` moves an install into the app's files; LaunchAgents are listed, not swapped

- **Node:** moves a pasture install into this repository: the token into its `.env`,
  `config.json`, `data/state.json` and `data/shell-state.json`, then replaces the
  LaunchAgents (back up, `launchctl bootout`, write and load `local.bellwether.*`, wait for
  the new brain through `brain/probe.mjs`, roll back on failure), then the hook entries.
- **Rust:** the same steps and order into the app's folder (`BELLWETHER_HOME`): the token
  into `hook-token` (from the old `.env`'s `BELLWETHER_HOOK_TOKEN` or `PASTURE_HOOK_TOKEN`,
  else from the hook entries, plan D9), `config.json` without the keys the app no longer
  reads, `state.json` through the brain's load checks, `shell-state.json`. The repository's
  LaunchAgents (`local.bellwether.*` as well as pasture's, found by what they run) are named
  and left alone: the app never writes or boots out a LaunchAgent (plan D5), and moving them
  to its login items is milestone 8. When HOME is the real home, the hook entries are
  rewritten only after a brain on the port accepts the token under `X-Bellwether-Token` (a
  doctor ping); under any other HOME (a test's) there is no ping and they are rewritten
  directly, as the Node CLI does under a temp HOME. Without `--from`, the old install is the WorkingDirectory of
  `local.bellwether.brain` or `local.pasture.brain` (plan D9).
- **Pinned by:** `test/migrate.test.mjs`, "the CLI moves an install, end to end" and "step
  by step" (both brains, each asserting where its files go; with no token in any `.env` the
  app takes the entries' token where Node leaves the hooks); `crates/brain/tests/
  migrate_cli.rs` (the ping, then the rewrite, against a Rust brain that takes the token,
  one that holds another, and none); `test/tester-migrate.test.mjs` and
  `test/tester-migrate-recheck.test.mjs`, the CLI previews; `crates/brain/src/migrate.rs`,
  `sources_and_obsolete_keys` and `old_agents_are_found_by_what_they_run`.

### 18. `pets import` is the app's; `pets fetch` is the repository's

- **Node:** `pets fetch` downloads the pinned eSheep art into `data/pets/`; there is no
  `pets import`.
- **Rust:** `pets import <animations.xml> [--name] [--force]` checks a pet file of the
  user's (16 MiB, UTF-8, no DOCTYPE or ENTITY, well-formed, `<animations>` root, `tilesx` and
  `tilesy` from 1 to 64, a PNG, an animation with an id), copies it to
  `pets/<name>/animations.xml`, pins its sha256 in `pets.user.lock.json` and proposes an
  `animationMap` by name (plan D8). `pets fetch` says the app doesn't redistribute the art
  (exit 3; Q6 keeps it a repository opt-in).
- **Pinned by:** `test/pets.test.mjs`, "pets import" (each brain its own answer);
  `crates/brain/src/pets.rs` tests.

### 19. `shell start|stop|restart` control Bellwether.app's strip

- **Node:** starts, stops and restarts the repository's strip (its LaunchAgent, or a
  detached copy of `target/release/bellwether-shell`); `shell --dev` builds it.
- **Rust (since milestone 5):** the arguments are checked as Node checks them (`shell bogus`,
  `--dev`, `--release`: exit 3). The actions control the strip of the app the CLI belongs
  to: `start` opens it through LaunchServices (`open -g -a <bundle>`, passing
  `BELLWETHER_HOME` on) unless its strip already runs; `stop` sends SIGTERM to that
  bundle's running strip, found by the exact executable path (never another copy, never a
  repository's `bellwether-shell`), and waits up to 5 s; `restart` is both. A brain started
  by launchd keeps running; a brain the strip started stops with it. Outside an app bundle
  (a development binary) they refuse with exit 2 and point at the repository's
  `./bellwether shell …`. `shell dock-permission` says it becomes `permissions
  accessibility` (milestone 6).
- **Pinned by:** `test/shellctl.test.mjs`, "`bellwether shell bogus` and `shell start
  --dev` are refused" and "`shell stop` outside the app" (each brain its own answer);
  `shell/src-tauri/src/cli.rs`, `unknown_commands_exit_1_and_bad_flags_exit_3`. The app's
  start, stop and restart were run on a signed test bundle (plan, milestone 5 status).

### 20. `token rotate` rewrites the hook entries

- **Node:** has no `token rotate`.
- **Rust:** `token rotate --write` writes the token file, tells a running brain (entry 7),
  then rewrites bellwether's hook entries with the new token through the installer (backup,
  hash check, exact-URL ownership). The entries change only once the brain switched, or
  when no brain answers at all; a brain that answers and doesn't switch (the repository's
  Node brain has no `reload-token`) gets the old token back in the file, the entries
  untouched, and exit 1. A settings file that can't be rewritten stops the rotation first; a
  failed rewrite puts the old token back in the file and tells the brain again, and says
  which of the three is off if that fails.
- **Pinned by:** `crates/brain/tests/token_cli.rs`, `token_init_and_rotate`;
  `crates/brain/tests/rotate_cli.rs` (a refusing brain, none, a failed write, a failed
  switch back).

### 21. Small wording differences of the installer

- The "not laid out the way bellwether writes JSON" summary names the app's backup folder
  (`<home>/backups/`) where Node says `data/backups/`.
- A missing token says "BELLWETHER_HOOK_TOKEN is not set, and `<home>/hook-token` doesn't
  exist or is empty. Make one with `bellwether token init`" (Node points at its `.env`).
- A JSON error names the position in Rust's words, not V8's (`is not valid JSON (…)` either
  way); the differential counts two refusals as the same answer.
- Every other test matches on the cause, never on these words.
- **Pinned by:** `test/hooks.test.mjs`, "the \"not laid out\" summary names the backup folder,
  and a missing token names BELLWETHER_HOOK_TOKEN" (each brain its own wording).

### 22. A settings file nested more than 1000 levels deep is refused

- **Node:** `JSON.parse` and the installer take a settings file nested about 2000 levels
  deep (5000 fails with "Maximum call stack size exceeded").
- **Rust:** the lossless JSON stops at 1000 levels, so nothing can overflow the stack: `is
  not valid JSON (nested too deeply …)`, exit 3, nothing written. No Claude Code settings
  file is that deep.
- **Pinned by:** `test/hooks.test.mjs`, "a settings file nested 1100 deep" (each brain its
  own answer); `crates/core/src/lossless.rs`, `bad_json_is_refused`.

## Turned out not to differ

- **Lone surrogates in hook payloads** (listed as expected for M3). The Rust brain keeps every
  JS string as UTF-16 code units (crates/core `jsstr.rs`), parses `"\ud800"` like
  `JSON.parse` and writes it back like `JSON.stringify`, so a lone surrogate reaches a bubble,
  a recording and `/state` exactly as in Node. Pinned by the redact and normalise differentials
  (their generators include lone surrogates) and `crates/core/src/json.rs`.

## Known differences from Node itself, shared by both brains

The `.env` reader (`brain/envfile.mjs`, and `settings.rs`'s `parse_env_value`, which
matches it on every shared vector and on 10⁵ generated cases) was written to read one
key the way `node --env-file` does. The differential's oracle check, which runs a sample
through `node --env-file` (Node 22.14) itself, shows two shapes where it doesn't:

- **A quoted value left open on its line:** Node reads on across lines until the quote
  closes (`KEY="a⏎b"` is `a⏎b`). The shared reader stops at the end of the line
  (`"a`).
- **A line with no key, or no `=`** (`=abc`, a bare word): Node stops reading the file
  there, so a `BELLWETHER_HOOK_TOKEN` further down is not set. The shared reader skips the
  line and reads on.

Neither shape is in a `.env` that `token init` wrote, and the app doesn't write `.env`
files (plan D3), so this matters only for importing a legacy token (milestone 3). Found in
milestone 2. `scripts/differential.mjs` reports these two shapes as known and fails on
any other difference from `node --env-file`.

## Expected with the port, not yet in effect

From plan D2 and R2. Each becomes an entry above, with its test, in the milestone that makes
it. Milestone 4 made the last three: integer-like keys (16), escapes (16, the same as Node),
and doctor without its `node` check (14). None is left.
