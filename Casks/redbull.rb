cask "redbull" do
  arch arm: "arm64", intel: "x86_64"

  version "0.2.5"
  sha256 arm:   "a36a679d1eccef51c8339ff6903dacedb120ae8dd1061c5f1217495a5939214e",
         intel: "dbed83298d5bee1040b4bf51ba4fd7574d175238bf1ec0f3b6360a5ae4a82174"

  url "https://github.com/tsgates/redbull/releases/download/v#{version}/Redbull-#{version}-#{arch}.dmg"
  name "Redbull"
  desc "Menu-bar app that keeps your Mac awake"
  homepage "https://github.com/tsgates/redbull"

  app "Redbull.app"

  zap trash: "~/Library/Preferences/com.redbull.stayawake.plist"
end
