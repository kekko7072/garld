# Homebrew formula for the garld command-line tool.
#
# Installs the `garld` CLI and the `garld-gui` launcher, built from source. For
# the drag-and-drop macOS application, use the cask instead:
#
#     brew install --cask kekko7072/garld/garld
#
# This file is the source of truth; the tap copy is generated from it by
# packaging/homebrew/sync-tap.sh, which fills in the version and checksum.
class Garld < Formula
  desc "GitHub Action Runner Local Dashboard - local runners, jobs and host metrics"
  homepage "https://github.com/kekko7072/garld"
  url "https://github.com/kekko7072/garld/archive/refs/tags/vVERSION.tar.gz"
  sha256 "SHA256"
  license "MIT"
  head "https://github.com/kekko7072/garld.git", branch: "main"

  depends_on "rust" => :build

  on_linux do
    depends_on "gtk+3" => :build
    depends_on "libxdo" => :build
  end

  def install
    system "cargo", "install", *std_cargo_args(path: ".")
  end

  test do
    # `info` needs no runners present and no network, so it is a real check that
    # the binary starts and can read the host.
    assert_match "hostname", shell_output("#{bin}/garld info")
    assert_match version.to_s, shell_output("#{bin}/garld --version")
  end
end
