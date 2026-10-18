# Changelog

## Unreleased

- **Move a sheep by dragging it.** Press and move, and it dangles from the cursor; let go
  and it lands there, on the strip of whichever monitor you dropped it on. It never lands
  on another sheep (it walks to the nearest clear spot), and its lambs follow. The move
  lasts until Bellwether restarts or the session ends; nothing is saved.
- A click on a sheep now counts when you let go, so a press can become a drag.
- With two monitors at different display scaling, hover labels, fading and clicks now
  line up with the sheep on the second monitor.

## 1.0.0

Bellwether puts a small flock of desktop sheep above your Dock, one per Claude Code
session. Each sheep shows what its session is doing, and the one that needs you turns to
face you with a bubble saying what it wants. 1.0 is the first stable release.

### Since 0.2.0-rc.3

- **The project label no longer sticks above a sheep.** Hovering a sheep shows its project;
  the label used to stay on screen (and follow the sheep) when the cursor left between two
  mouse events, when the sheep walked out from under a still cursor, after *Hide
  everything*, *Quiet* or *Show labels*, or when the session ended. It now goes whenever
  the cursor is no longer on the sheep (first in 1.0.0-rc.1).
- The Node brain and the repository install were removed from the source: the app has run
  only the Rust brain since 0.2.0-rc.1. Contributors run the Rust binary from a checkout
  (docs/DEVELOPMENT.md).
- A pet file that doesn't match its pin now says how to fix it in a development build's
  terms; nothing else the app shows changed.

### Since 0.1 (the repository version, run from a checkout with Node)

- A signed, notarized app (the DMG, or `brew install --cask fanzoo/tap/bellwether`) with
  no Node, and a `bellwether` command on your PATH.
- The setup window, start at login through System Settings, and updates from the menu
  (*Check for Updates…*).
- Bellwether's own sheep in a flight helmet (CC0), and `bellwether pets import` in place
  of `pets fetch`.
- The hook token in the app's `hook-token` file, and `bellwether token rotate`.
- Every `config.json` key is optional.

Behaviour changes worth knowing:

- `bubble.redactPatterns` with look-around or backreferences are refused: the app's
  regular expressions don't have them (a config error naming the pattern).
- `PASTURE_*` names and `X-Pasture-*` headers are no longer read.
- `bellwether doctor`'s lines are the app's: login items, the app's signature, the strip's
  Accessibility, leftover LaunchAgents.

### Coming from a repository install

- Install the app, then use the setup window's *Move it into the app*, or run
  `bellwether migrate --to-app` (a preview) and then `bellwether migrate --to-app
  --write`. `bellwether migrate --undo --write` reverses it.
- Do this **before** pulling v1.0.0. A checkout at 1.0 has no `./bellwether`, so its
  LaunchAgent brain can't start again.
- If you already pulled, the migration still works: it reads only the old LaunchAgent
  plists, `.env`, `config.json` and `data/`.
- An undo needs the old brain back: `git checkout v0.2.0-rc.3` in the checkout first.

## 1.0.0-rc.1

A release candidate for 1.0.0, so the hover-label fix runs on real strips before 1.0.0.

- **The project label no longer sticks above a sheep.** It goes whenever the cursor is no
  longer on the sheep: when the cursor leaves, when the sheep walks away, after *Hide
  everything*, *Quiet* or *Show labels*, and when the session ends. The strip learns where
  the cursor is from the same check that decides which clicks it takes, so no new
  permission is asked.
- The Node brain and the repository install were removed from the source. If your Mac
  still runs Bellwether from a checkout, see "Coming from a repository install" under
  1.0.0 above.
