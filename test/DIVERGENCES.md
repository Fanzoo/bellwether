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

### 3. `/health` counters include `dropped`

- **Node:** has no such counter.
- **Rust:** `counters.dropped` counts events the state machine couldn't take in time (plan
  D1). It is always 0 until milestone 3 brings the state machine.
- **Pinned by:** `crates/brain/tests/http.rs`, `health_carries_no_content`.

### 4. `X-Pasture-*` headers are not accepted

- **Node:** for one release after the rename, a hook carrying `X-Pasture-Token` (and the
  `X-Pasture-*` terminal headers) is accepted (`test/rename.test.mjs`, "a running brain
  accepts a hook sent by an entry pasture wrote").
- **Rust:** refused with an empty 403, like any missing token. Maintainer decision 14 ends
  the legacy acceptance with the last repository release.
- **Pinned by:** `crates/brain/src/hook.rs`, `the_guards_in_order` ("the pre-rename
  header is not accepted"), and `crates/brain/tests/http.rs`,
  `hook_answers_are_status_only_and_the_guards_refuse`.

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
- **Lone surrogates in hook payloads (M3).** JSON.parse accepts a lone `\ud800` escape;
  serde_json refuses it.
- **User `bubble.redactPatterns` with look-around or backreferences (M3).** A config
  error (exit 3) naming the pattern, since `regex` has neither.
- **`token init` writes the token file, not `.env` (M3),** and the in-place `.env` writer
  with its tests is retired with the Node CLI.
- **Doctor's `node` check is removed (M4).**
