<div align="center">

<img src="docs/icon.png" width="120" alt="Redbull icon">

# Redbull

**A tiny macOS menu-bar app that keeps your Mac awake.**

One slider — from 15 minutes to forever. No windows, no fuss.

[![Latest release](https://img.shields.io/github/v/release/tsgates/redbull?color=E24B4A)](https://github.com/tsgates/redbull/releases/latest)
[![Downloads](https://img.shields.io/github/downloads/tsgates/redbull/total?color=E24B4A)](https://github.com/tsgates/redbull/releases)
![Platform](https://img.shields.io/badge/macOS-10.13%2B-555)
![Built with Rust](https://img.shields.io/badge/built%20with-Rust%20%F0%9F%A6%80-orange)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)

[**Website**](https://tsgates.github.io/redbull/) · [**Download**](https://github.com/tsgates/redbull/releases/latest)

</div>

---

Redbull lives in your menu bar as a lightning bolt. Click it, drag the slider to a
duration, and your Mac stays awake until the timer runs out. It's a clean front-end
for the system `caffeinate` tool — **~100 KB**, native Rust + AppKit, **~0% idle CPU**,
no daemon, no login, no Electron.

## Install

### Homebrew

```sh
brew install --cask tsgates/tap/redbull
```

### Direct download

1. Grab the `.dmg` for your chip (**Apple Silicon** or **Intel**) from the
   [releases page](https://github.com/tsgates/redbull/releases/latest).
2. Open it and drag **Redbull** into Applications.
3. First launch: right-click → **Open** once (the app is ad-hoc signed), then click
   the ⚡ in your menu bar.

### Build from source

```sh
git clone https://github.com/tsgates/redbull
cd redbull
cargo run                # quick run (debug)
./package.sh             # build Redbull.app (native, stable Rust)
./release.sh             # size-optimized arm64 + x86_64 .dmg/.zip (nightly)
```

## Features

- **One slider** — drag from `Off` to `∞`. The whole control is a single tick slider.
- **Sensible presets** — 15 m, 1 / 2 / 3 / 6 / 12 h, or indefinitely.
- **Live countdown** — the menu-bar bolt shows time remaining, fixed-width so it never jitters.
- **Featherweight** — a ~100 KB native binary; ~0% CPU when idle.
- **Zero config** — no account, no preferences, no login item.
- **Open source** — built on `caffeinate`; read every line.

## How it works

Picking a duration runs the system power-assertion tool:

```sh
caffeinate -d -i -t 7200   # hold display + idle sleep for 2 hours
```

Display and idle sleep are held for the chosen time, then released automatically.
The UI is built directly on AppKit via [`objc2`](https://github.com/madsmtm/objc2):
the app owns an `NSStatusItem` and anchors an `NSPopover` (a `WKWebView` rendering
the slider) to it.

## Releasing

Tag a version and the [GitHub Action](.github/workflows/release.yml) builds both
arch packages and publishes a release:

```sh
# bump `version` in Cargo.toml, then:
git tag v0.2.0 && git push origin v0.2.0
```

## License

[MIT](LICENSE)
