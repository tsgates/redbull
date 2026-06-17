cask "redbull" do
  arch arm: "arm64", intel: "x86_64"

  version "0.2.0"
  sha256 arm:   "2ea86fd987c8f1292ff08ed1a07fc036ba2906ea1844fb8e8d3566316432cda9",
         intel: "b12081881448504d811c9b6c8176762d4304900f7eca3ac5d901d034c5250cd1"

  url "https://github.com/tsgates/redbull/releases/download/v#{version}/Redbull-#{version}-#{arch}.dmg"
  name "Redbull"
  desc "Menu-bar app that keeps your Mac awake"
  homepage "https://github.com/tsgates/redbull"

  app "Redbull.app"

  zap trash: "~/Library/Preferences/com.redbull.stayawake.plist"
end
