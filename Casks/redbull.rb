cask "redbull" do
  arch arm: "arm64", intel: "x86_64"

  version "0.2.1"
  sha256 arm:   "073e9fddfdf7bb4640f864c2675d7c8af210d8cc805c23d6a85e03907d18e22b",
         intel: "3b53171b48491d3b4bcbdb406bfa23b1637b64da6ae6d9cc0d655dfd6ac2b62b"

  url "https://github.com/tsgates/redbull/releases/download/v#{version}/Redbull-#{version}-#{arch}.dmg"
  name "Redbull"
  desc "Menu-bar app that keeps your Mac awake"
  homepage "https://github.com/tsgates/redbull"

  app "Redbull.app"

  zap trash: "~/Library/Preferences/com.redbull.stayawake.plist"
end
