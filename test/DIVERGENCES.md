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
  over the .env".
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
  listing what the app has (no `shell`, `hooks`, `pets` or `migrate` yet; those come with
  milestones 4–6).
- **Pinned by:** `crates/brain/src/cli.rs`, `ports`; `shell/src-tauri/src/cli.rs`,
  `help_lists_the_brain_commands` and `unknown_commands_exit_1_and_bad_flags_exit_3`
  (`version`); `test/hook-route-blackbox.test.mjs` starts every Rust brain with
  `serve --port`.

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
it.

- **Integer-like object keys keep file order (M4).** `JSON.parse` moves keys like `"1"`
  and `"2"` before the others, so a Node install and uninstall of a file with such keys
  doesn't give back its bytes. The lossless JSON module keeps file order. The Node
  behaviour is in `test/fixtures/installer-vectors/exotic-numbers-escapes-and-integer-like-keys/`
  (`roundTrip: false`).
- **Escapes in settings files (M4).** Node rewrites `\/`, `é` and surrogate-pair
  escapes as the characters they stand for (the same vector). Whether the port keeps or
  rewrites them is decided in M4.
- **Doctor's `node` check is removed (M4).**
