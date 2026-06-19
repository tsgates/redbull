cask "redbull" do
  arch arm: "arm64", intel: "x86_64"

  version "0.2.4"
  sha256 arm:   "a18c962ddb5053111c76fc5d3f584f6a61bfafd372dd857f188f43221588a70a",
         intel: "7e5661363aad8699265941510256bf04640f67daa92a73620624e5510e18579d"

  url "https://github.com/tsgates/redbull/releases/download/v#{version}/Redbull-#{version}-#{arch}.dmg"
  name "Redbull"
  desc "Menu-bar app that keeps your Mac awake"
  homepage "https://github.com/tsgates/redbull"

  app "Redbull.app"

  zap trash: "~/Library/Preferences/com.redbull.stayawake.plist"
end
