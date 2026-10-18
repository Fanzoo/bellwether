# Releasing Bellwether

How a version of Bellwether.app gets from a commit to users: a draft GitHub release built
by CI, published by hand, then the app's updater and the Homebrew tap pick it up
(distribution plan D7, milestone 9).

| Piece | Where |
|---|---|
| Release workflow (a pushed `v*` tag → a draft release) | `.github/workflows/release.yml` |
| Homebrew tap pull request (a published release → a PR on the tap) | `.github/workflows/tap.yml` |
| Build, sign, notarize, staple the app | `scripts/build-app.mjs` |
| Version bump, release checks, DMG, updater archive, `latest.json`, `SHA256SUMS`, cask | `scripts/release.mjs` |
| The cask template | `packaging/homebrew/bellwether.rb` |
| The updater's feed and public key | `shell/src-tauri/tauri.dist.conf.json` (`plugins.updater`) |
| The app's side | `shell/src-tauri/src/updater.rs`, the tray's *Check for Updates…* |
| Measuring the brain's gap during an update | `scripts/health-gap.mjs` |

Nothing here needs `.env`. No secret is ever written into the repository: the signing
material lives in GitHub environments and in your own offline backup.

## One-time setup

Do these once, in order. Steps 1 and 2 reuse what milestone 1 already set up for local
notarization if you still have the files.

0. **Tools, in a checkout of the repository** (a fresh clone needs all of these):
   - Node 22.9 or later (`node --version`), Xcode's command-line tools
     (`xcode-select --install`), and Rust (`rustup`; the suites in "Each release" need it).
   - The Tauri CLI, pinned with hashes by `shell/package-lock.json`, into
     `shell/node_modules` (gitignored): `npm ci --prefix shell`. Step 3 runs it.
   - The GitHub CLI, signed in as an admin of Fanzoo/bellwether (`brew install gh`, then
     `gh auth login`); steps 5 and 7 use it to set secrets. Check with `gh auth status`.
   - For the suites on a fresh clone, what CI does first: `cargo build --manifest-path
     shell/src-tauri/Cargo.toml` and `node brain/cli.mjs pets fetch`.

1. **Developer ID Application certificate, as a `.p12`.** In Keychain Access, find
   "Developer ID Application: … (your Team ID)" under My Certificates, export it with its
   private key as `.p12` with a strong password. Keep it outside the repository.
2. **App Store Connect API key for notarization.** App Store Connect > Users and Access >
   Integrations > App Store Connect API: a key with the Developer role. Note the
   **Issuer ID** and **Key ID**, and download `AuthKey_<id>.p8` (it downloads once; if it
   is lost, revoke the key and make another).
3. **The updater's signing key.** On your Mac, outside the repository:

   ```
   mkdir -p ~/BellwetherRelease && chmod 700 ~/BellwetherRelease
   shell/node_modules/.bin/tauri signer generate -w ~/BellwetherRelease/updater.key
   chmod 600 ~/BellwetherRelease/updater.key
   ```

   It asks for a password; use a strong one. It writes `updater.key` (private) and
   `updater.key.pub` (public). **Back up `updater.key` and its password offline, in two
   places** (a password manager plus an encrypted drive, say). Losing either means
   installed copies can never be updated again (plan R9); there is no recovery.
4. **Put the public key in the repository.** Paste the single line of
   `~/BellwetherRelease/updater.key.pub` (**`.pub`**: the repository is public) as
   `plugins.updater.pubkey` in `shell/src-tauri/tauri.dist.conf.json`, replacing the
   `SET-ME: …` text. Run `node scripts/release.mjs check v<the current version>` before you
   commit; the only complaint left should be the version, if you haven't bumped it. It
   refuses the secret `updater.key` pasted by mistake, and any file in the checkout that
   holds a minisign secret key (`npm test` and CI check the same). If a secret key was ever
   committed or pushed, treat it as public: make a new key pair. Then commit.
5. **The `release` environment.** github.com/Fanzoo/bellwether > Settings > Environments >
   New environment, named `release`:
   - Required reviewers: you. Leave "Prevent self-review" off (you are the only reviewer).
   - Deployment branches and tags: Selected branches and tags > Add rule > **Tag**, pattern
     `v*`.
   - Environment secrets (from a terminal in a folder outside the repository; `gh secret
     set` reads the value from stdin, so nothing lands in your shell history):

     ```
     base64 -i DeveloperID.p12 | gh secret set APPLE_CERTIFICATE --env release -R Fanzoo/bellwether
     gh secret set APPLE_CERTIFICATE_PASSWORD --env release -R Fanzoo/bellwether        # paste, Enter, Ctrl-D
     gh secret set APPLE_SIGNING_IDENTITY --env release -R Fanzoo/bellwether            # the full name as `security find-identity -v -p codesigning` prints it, or its SHA-1
     gh secret set APPLE_API_KEY --env release -R Fanzoo/bellwether                     # the Key ID
     gh secret set APPLE_API_ISSUER --env release -R Fanzoo/bellwether                  # the Issuer ID
     base64 -i AuthKey_XXXXXXXXXX.p8 | gh secret set APPLE_API_KEY_P8 --env release -R Fanzoo/bellwether
     gh secret set TAURI_SIGNING_PRIVATE_KEY --env release -R Fanzoo/bellwether < ~/BellwetherRelease/updater.key
     gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --env release -R Fanzoo/bellwether
     ```

   Check that no repository-level secret has any of these names (Settings > Secrets and
   variables > Actions > Repository secrets should hold none of them).
6. **Actions settings.** Settings > Actions > General: Workflow permissions "Read
   repository contents and packages permissions" (the workflows ask for more only where
   they need it); fork pull request workflows "Require approval for first-time
   contributors" (the default). Fork pull requests never receive secrets.
7. **The Homebrew tap.**
   - Create the public repository **Fanzoo/homebrew-tap** (default branch `main`, with a
     README so it isn't empty). The workflow adds `Casks/bellwether.rb`.
   - A fine-grained personal access token: Settings > Developer settings > Fine-grained
     tokens > Generate. Resource owner Fanzoo; Repository access **Only select
     repositories: Fanzoo/homebrew-tap**; Permissions **Contents: Read and write**, **Pull
     requests: Read and write**, nothing else; an expiry you will notice (90 days, say).
   - In github.com/Fanzoo/bellwether > Settings > Environments, a second environment
     `homebrew-tap` with the same tag rule (`v*`) and one secret:
     `gh secret set TAP_GITHUB_TOKEN --env homebrew-tap -R Fanzoo/bellwether`.

## Each release

1. Bump the version everywhere it lives, run the suite, commit:

   ```
   node scripts/release.mjs version 0.2.0-rc.1
   npm test && npm run test:cargo
   git commit -am "Release 0.2.0-rc.1"
   node scripts/release.mjs check v0.2.0-rc.1      # versions, bundle id, feed and key agree
   ```

2. Tag the commit you just pushed to main, and push the tag: `git push origin main`, then
   `git tag v0.2.0-rc.1 && git push origin v0.2.0-rc.1`.
3. In the Actions tab, the `release` run waits for your approval (the environment's
   reviewer). **Before approving**, open the run and check that its commit is the one you
   tagged on `main` (a `v*` tag can point at any commit, and that commit's workflow and
   scripts are what get the signing secrets). The job also refuses, before any secret is
   used, a tag whose commit isn't on `main`. Approve it. It builds the universal app, signs, notarizes and staples it,
   makes the DMG and notarizes and staples that too, signs the updater archive, and drafts
   the release with the DMG, `Bellwether_<v>_universal.app.tar.gz` and `.sig`,
   `latest.json` and `SHA256SUMS`.
4. Check the draft before publishing. Download its files into an empty folder:

   ```
   shasum -a 256 -c SHA256SUMS
   spctl --assess --type open --context context:primary-signature -vvv Bellwether_0.2.0-rc.1_universal.dmg
   xcrun stapler validate Bellwether_0.2.0-rc.1_universal.dmg
   ```

   `spctl` must say `accepted` and `source=Notarized Developer ID`. (The publish job's own
   `sha256sum -c` only proves the files weren't corrupted between the two jobs: the
   `SHA256SUMS` it checks came in the same artifact. What makes the files trustworthy is
   that only the reviewed, environment-gated build job made them; these checks of yours are
   the ones against Apple's notarization.)
5. Publish: edit the draft, leave **Set as a pre-release unchecked** and **Set as the
   latest release checked**, then Publish. The updater reads
   `releases/latest/download/latest.json`, and GitHub's "latest" skips drafts and
   pre-releases, so a pre-release never reaches installed apps (useful when you want an rc
   only for yourself: publish it as a pre-release and install its DMG by hand). **Publish in
   version order**: whichever release was published last as "latest" is what every
   installed app is offered, so publishing an older patch after a newer version would hide
   the newer one.
6. A full release (not a pre-release) runs `tap.yml`: approve its `homebrew-tap` deployment
   if you gave that environment a reviewer; it opens a PR on Fanzoo/homebrew-tap. Merge it.
   A pre-release gets no tap PR; if you later turn it into a full release, the PR opens
   then.

## What Homebrew's uninstall does

`brew uninstall --cask bellwether` quits the app, stops the brain agent and removes the app
and the `bellwether` link. Your files stay. `brew uninstall --zap --cask bellwether` also
deletes `~/Library/Application Support/Bellwether` (your **config.json**, the hook token,
state, colours and imported pets), the logs and the app's preferences and caches. Run
`bellwether hooks uninstall --write` and `bellwether autostart uninstall --write` first
either way: Homebrew can't edit Claude Code's settings. Afterwards, `launchctl list | grep
fanzootechnology` should print nothing.

## Proving milestone 9 (the "Done when" items only you can do)

1. **A tagged draft.** Do "Each release" steps 1–4 for `v0.2.0-rc.1`. Done when step 4's
   `spctl` accepts the DMG and the draft has the DMG, the archive and `.sig`,
   `latest.json` and `SHA256SUMS`.
2. **No secret in the log.** In a terminal, without echoing the values:

   ```
   gh run view <run id> -R Fanzoo/bellwether --log > /tmp/release-log.txt
   for name in "certificate password" "API key id" "issuer id" "Team ID"; do
     printf '%s: ' "$name"; read -rs v; echo
     grep -cF -- "$v" /tmp/release-log.txt
   done
   rm /tmp/release-log.txt
   ```

   Every count must be 0 (GitHub shows masked values as `***`).
3. **Install rc.1.** Publish rc.1 as the latest release (step 5). Quit any running
   Bellwether, drag the DMG's app to /Applications, open it, and in the setup window turn
   on start at login, grant Accessibility (sheep on the Dock) and Automation for your
   terminal (click a sheep). Check `bellwether doctor` (or
   `/Applications/Bellwether.app/Contents/MacOS/Bellwether doctor`).
4. **Release rc.2** the same way (`version 0.2.0-rc.2`, commit, tag, approve, check,
   publish as latest).
5. **Update through the menu.** In a terminal: `node scripts/health-gap.mjs --port 47417
   --seconds 300`. Then in the menu-bar sheep: *Check for Updates…* > *Install and
   Relaunch*. Done when:
   - the app comes back as the new version (`bellwether version`;
     `~/Library/Logs/Bellwether/shell.log` has `updates: … installed; relaunching` and then
     `… was this app's login agent at version <old> (pid N); restarted it (launchctl
     kickstart -k): now version <new> …`);
   - without any manual `launchctl kickstart`, `bellwether doctor`'s brain line is a PASS
     showing the new version and a new pid (a WARN saying "still running <old>" is this
     check failing);
   - `health-gap.mjs` shows the new version within 2 s of the relaunch and its gap line
     under 1000 ms;
   - Accessibility and Automation still hold: `bellwether doctor`'s accessibility line says
     granted to the strip, sheep sit on the Dock, and clicking a sheep still focuses its tab with no new
     prompt.
6. **Homebrew.** After the tap PR for rc.2 is merged, on a Mac or user account without
   /Applications/Bellwether.app (or after moving it to the Trash):
   `brew install --cask fanzoo/tap/bellwether`, then in a new terminal `which bellwether`
   (Homebrew's bin) and `bellwether doctor`.
7. **A fork's pull request.** From another GitHub account (or ask someone), fork
   Fanzoo/bellwether and open a PR with a trivial change. Done when `ci` runs on it (after
   you approve a first-time contributor's run) and Settings > Environments > `release`
   lists no deployment for it; the PR's checks show no `release` or `tap` workflow.

## Testing the updater locally (no keys of yours involved)

The endpoint override exists only in debug builds. Everything below lives under /tmp and
uses a key made for the test and deleted afterwards. Use a free port other than 47417.

Needs one-time step 0's tools (`npm ci --prefix shell` included) and `rustup target add
aarch64-apple-darwin`.

```
KD=$(mktemp -d /tmp/bw-keys.XXXXXX)
shell/node_modules/.bin/tauri signer generate --ci -p test -w $KD/throwaway.key
# a copy of the checkout to build two versions from (never .env, config.json or data/)
rsync -a --exclude .git/ --exclude .env --exclude '.env.*' --exclude /config.json --exclude /data/ \
  --exclude node_modules/ --exclude shell/src-tauri/target/ --exclude shell/src-tauri/target-dev/ ./ /tmp/bw-src/
ln -s "$PWD/shell/node_modules" /tmp/bw-src/shell/node_modules
cd /tmp/bw-src
node scripts/release.mjs version 0.2.0-rc.1
node scripts/build-app.mjs --adhoc --debug --test-port 47561 --test-home /tmp/bw-home --updater-pubkey $KD/throwaway.key.pub
mkdir -p /private/tmp/bw-run && ditto shell/src-tauri/target-dev/debug/bundle/macos/Bellwether.app /private/tmp/bw-run/Bellwether.app
node scripts/release.mjs version 0.2.0-rc.2
node scripts/build-app.mjs --adhoc --debug --test-port 47561 --test-home /tmp/bw-home --updater-pubkey $KD/throwaway.key.pub
TAURI_SIGNING_PRIVATE_KEY_PATH=$KD/throwaway.key TAURI_SIGNING_PRIVATE_KEY_PASSWORD=test \
  node scripts/release.mjs assets --app shell/src-tauri/target-dev/debug/bundle/macos/Bellwether.app \
  --tag v0.2.0-rc.2 --out /tmp/bw-srv --adhoc-dmg --base-url http://127.0.0.1:47562/
(cd /tmp/bw-srv && python3 -m http.server 47562 --bind 127.0.0.1) &      # stop it by its pid afterwards
BELLWETHER_HOME=/tmp/bw-home /private/tmp/bw-run/Bellwether.app/Contents/MacOS/Bellwether token init
printf '{"version": 1, "hidden": false, "showLabels": false, "onboarded": true}\n' > /tmp/bw-home/shell-state.json   # no setup window
node scripts/health-gap.mjs --port 47561 --seconds 60 &
HOME=/tmp/bw-userhome BELLWETHER_UPDATE_ENDPOINT=http://127.0.0.1:47562/latest.json \
  BELLWETHER_UPDATE_ANSWER=install BELLWETHER_UPDATE_FIRST_CHECK_SEC=8 \
  /private/tmp/bw-run/Bellwether.app/Contents/MacOS/Bellwether
rm -rf $KD
```

- Run the app from a path without symlinks (`/private/tmp`, not `/tmp`): the updater
  refuses an executable path that contains one.
- `BELLWETHER_UPDATE_ANSWER=install|later` answers the offer without a click (and skips
  the failure alert); leave it out to see the real alert.
- A `latest.json` whose `version` differs from the version the archive was signed for is
  refused ("The update was signed for version …"), and so is an archive the signature
  doesn't match ("The signature verification failed"); the app keeps running unchanged.
