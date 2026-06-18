cask "redbull" do
  arch arm: "arm64", intel: "x86_64"

  version "0.2.2"
  sha256 arm:   "59186f79c6e8089b3a98ce0462cd7c9e7f9ac9dca2fa8bc21b6d107b3ff04d5a",
         intel: "3f3b2b89456dfafae11c8a33755a708fd31f3b19ee68e4d64d45d6ce6c5ed303"

  url "https://github.com/tsgates/redbull/releases/download/v#{version}/Redbull-#{version}-#{arch}.dmg"
  name "Redbull"
  desc "Menu-bar app that keeps your Mac awake"
  homepage "https://github.com/tsgates/redbull"

  app "Redbull.app"

  zap trash: "~/Library/Preferences/com.redbull.stayawake.plist"
end
