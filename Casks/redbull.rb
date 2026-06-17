cask "redbull" do
  arch arm: "arm64", intel: "x86_64"

  version "0.1.0"
  sha256 arm:   "76b4ba7066804c0eba86d81c8306780484869761a6727691273c4b317e9802c9",
         intel: "a1f185b13278e06d434f4735a1894db335dbb8cc44014b74d01005df3d4f425e"

  url "https://github.com/tsgates/redbull/releases/download/v#{version}/Redbull-#{version}-#{arch}.dmg"
  name "Redbull"
  desc "Menu-bar app that keeps your Mac awake"
  homepage "https://github.com/tsgates/redbull"

  app "Redbull.app"

  zap trash: "~/Library/Preferences/com.redbull.stayawake.plist"
end
