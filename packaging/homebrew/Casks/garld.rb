# Homebrew cask for the garld macOS application.
#
#     brew install --cask kekko7072/garld/garld
#
# Installs garld.app from the release disk image. Generated from this file by
# packaging/homebrew/sync-tap.sh, which fills in the version and checksum.
#
# Deliberately no `binary` stanza. The bundle is signed ad-hoc rather than
# notarised, and Homebrew quarantines cask downloads, so a symlink onto PATH
# pointing inside the bundle stalls in dyld on first execution while Gatekeeper
# waits for a confirmation that a shell cannot show. The formula builds the same
# CLI from source with none of that, so that is where the command line lives.
cask "garld" do
  version "VERSION"

  # The disk image carries a universal binary, so one checksum covers both
  # Apple silicon and Intel.
  sha256 "SHA256_DMG"

  url "https://github.com/kekko7072/garld/releases/download/v#{version}/garld-#{version}-macos.dmg"
  name "garld"
  desc "GitHub Action Runner Local Dashboard"
  homepage "https://github.com/kekko7072/garld"

  depends_on macos: :big_sur

  app "garld.app"

  caveats <<~CAVEATS
    garld is signed ad-hoc, not notarised by Apple, so macOS asks you to confirm
    the first launch. Open it from Finder with a right-click on garld in
    /Applications and choose Open, then confirm. Later launches are normal.

    For the `garld` command line tool, install the formula, which builds from
    source and needs no confirmation:

      brew install kekko7072/garld/garld
  CAVEATS

  zap trash: [
    "~/Library/Application Support/garld",
    "~/Library/Application Support/garld widget",
    "~/Library/Caches/garld",
    "~/Library/Saved Application State/io.github.kekko7072.garld.savedState",
  ]
end
