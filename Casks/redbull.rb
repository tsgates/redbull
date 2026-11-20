cask "redbull" do
  arch arm: "arm64", intel: "x86_64"

  version "0.2.6"
  sha256 arm:   "0bd03f69cc70c8fa235b7c1cf8a8d687b13be03c5b7530bb30a61af0175bd398",
         intel: "2c3eb8ef952f6b6b92284572e853d61c924df32cd0ac6c25fdf83b729da787bb"

  url "https://github.com/tsgates/redbull/releases/download/v#{version}/Redbull-#{version}-#{arch}.dmg"
  name "Redbull"
  desc "Menu-bar app that keeps your Mac awake"
  homepage "https://github.com/tsgates/redbull"

  app "Redbull.app"

  zap trash: "~/Library/Preferences/com.redbull.stayawake.plist"
end
