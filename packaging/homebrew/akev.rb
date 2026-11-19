# Homebrew formula for the akev command-line tool (rendered by scripts/package-manifests.sh).
class Akev < Formula
  desc "Fast HTTP(S) debugging proxy: intercept, inspect, mock and replay traffic"
  homepage "https://akev.dev"
  license "AGPL-3.0-or-later"

  on_macos do
    url "https://github.com/akevdev/akev/releases/download/v@VERSION@/akev-@VERSION@-universal-apple-darwin.tar.gz"
    sha256 "@SHA:akev-@VERSION@-universal-apple-darwin.tar.gz@"
  end

  on_linux do
    on_intel do
      url "https://github.com/akevdev/akev/releases/download/v@VERSION@/akev-@VERSION@-x86_64-unknown-linux-gnu.tar.gz"
      sha256 "@SHA:akev-@VERSION@-x86_64-unknown-linux-gnu.tar.gz@"
    end
    on_arm do
      url "https://github.com/akevdev/akev/releases/download/v@VERSION@/akev-@VERSION@-aarch64-unknown-linux-gnu.tar.gz"
      sha256 "@SHA:akev-@VERSION@-aarch64-unknown-linux-gnu.tar.gz@"
    end
  end

  def install
    bin.install "akev"
    doc.install "README.md", "CHANGELOG.md"
    (share/"licenses/akev").install "LICENSE"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/akev --version")
  end
end
