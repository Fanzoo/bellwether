# The Homebrew cask for Bellwether (distribution plan D7, milestone 9). This is the
# template: the tap workflow (.github/workflows/tap.yml) renders it for each published
# release with `node scripts/release.mjs cask` (the version and the DMG's sha256 replace
# the zeros below, and this comment gives way to a two-line header saying the file is
# generated) and opens a pull request on Fanzoo/homebrew-tap as Casks/bellwether.rb.
cask "bellwether" do
  version "0.0.0"
  sha256 "0000000000000000000000000000000000000000000000000000000000000000"

  url "https://github.com/Fanzoo/bellwether/releases/download/v#{version}/Bellwether_#{version}_universal.dmg"
  name "Bellwether"
  desc "Desktop sheep for your Claude Code sessions"
  homepage "https://github.com/Fanzoo/bellwether"

  livecheck do
    url :url
    strategy :github_latest
  end

  auto_updates true
  depends_on macos: :ventura

  app "Bellwether.app"
  binary "#{appdir}/Bellwether.app/Contents/MacOS/Bellwether", target: "bellwether"

  uninstall launchctl: "com.fanzootechnology.bellwether.brain",
            quit:      "com.fanzootechnology.bellwether"

  zap trash: [
    "~/Library/Application Support/Bellwether",
    "~/Library/Caches/com.fanzootechnology.bellwether",
    "~/Library/HTTPStorages/com.fanzootechnology.bellwether",
    "~/Library/Logs/Bellwether",
    "~/Library/Preferences/com.fanzootechnology.bellwether.plist",
    "~/Library/WebKit/com.fanzootechnology.bellwether",
  ]

  caveats <<~EOS
    Bellwether updates itself (Check for Updates… in its menu-bar menu).

    Homebrew can't edit Claude Code's settings. Before `brew uninstall --cask bellwether`,
    remove Bellwether's hooks and its login items:
      bellwether hooks uninstall --write
      bellwether autostart uninstall --write
    `brew uninstall --zap` also deletes ~/Library/Application Support/Bellwether,
    your config.json and hook token included.
  EOS
end
