# Homebrew cask for the garld macOS application.
#
#     brew install --cask kekko7072/garld/garld
#
# Installs garld.app from the release disk image, and links the bundled CLI so
# `garld` works in a terminal too. Generated from this file by
# packaging/homebrew/sync-tap.sh, which fills in the version and checksums.
cask "garld" do
  version "VERSION"

  # The disk image carries a universal binary, so one checksum covers both
  # Apple silicon and Intel.
  sha256 "SHA256_DMG"

  url "https://github.com/kekko7072/garld/releases/download/v#{version}/garld-#{version}-macos.dmg"
  name "garld"
  desc "GitHub Action Runner Local Dashboard"
  homepage "https://github.com/kekko7072/garld"

  depends_on macos: ">= :big_sur"

  app "garld.app"
  binary "#{appdir}/garld.app/Contents/MacOS/garld"

  zap trash: [
    "~/Library/Application Support/garld",
    "~/Library/Application Support/garld widget",
    "~/Library/Caches/garld",
  ]
end
