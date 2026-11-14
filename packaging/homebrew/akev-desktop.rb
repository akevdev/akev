# Homebrew cask for the Akev desktop app (rendered by scripts/package-manifests.sh).
cask "akev-desktop" do
  version "@VERSION@"
  sha256 "@SHA:Akev_@VERSION@_universal.dmg@"

  url "https://github.com/akevdev/akev/releases/download/v#{version}/Akev_#{version}_universal.dmg"
  name "Akev"
  desc "Fast HTTP(S) debugging proxy: intercept, inspect, mock and replay traffic"
  homepage "https://akev.dev"

  livecheck do
    url :url
    strategy :github_latest
  end

  # The app updates itself (signed packages, after the user confirms).
  auto_updates true
  depends_on macos: ">= :big_sur"

  app "Akev.app"

  zap trash: [
    "~/Library/Application Support/akev",
    "~/Library/Application Support/dev.akev.desktop",
    "~/Library/Caches/dev.akev.desktop",
    "~/Library/Preferences/dev.akev.desktop.plist",
    "~/Library/Saved Application State/dev.akev.desktop.savedState",
    "~/Library/WebKit/dev.akev.desktop",
  ]
end
