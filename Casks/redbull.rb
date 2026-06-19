cask "redbull" do
  arch arm: "arm64", intel: "x86_64"

  version "0.2.3"
  sha256 arm:   "2947e088ba2435085243dccd7bf09b5772eb3698554c4f89c464327f0baa62ee",
         intel: "0c7ad78fea193a984d06518f60ef52ee45237cc7c9f8cc0a60852d075dc64c14"

  url "https://github.com/tsgates/redbull/releases/download/v#{version}/Redbull-#{version}-#{arch}.dmg"
  name "Redbull"
  desc "Menu-bar app that keeps your Mac awake"
  homepage "https://github.com/tsgates/redbull"

  app "Redbull.app"

  zap trash: "~/Library/Preferences/com.redbull.stayawake.plist"
end
