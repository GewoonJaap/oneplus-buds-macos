# buds-ctl

Native macOS control for OnePlus Buds Pro 3 (HeyMelody features, without the phone app):
battery, noise control (Off / Transparency / Adaptive / Noise Cancellation High-Medium-Low),
equalizer presets and spatial audio. Changes made on the phone show up live.

- `buds` – CLI (`buds status`, `buds set <mode>`, `buds eq`, `buds spatial`, ...)
- `Buds.app` – menu bar app (SwiftUI panel + Rust Bluetooth daemon). Needs macOS 14+; Liquid Glass on macOS 26+.

## Install

Download the zip from Releases, unzip, move `Buds.app` to Applications. The app is ad-hoc signed, so
macOS will quarantine it; run once:

```bash
xattr -dr com.apple.quarantine /Applications/Buds.app
```

On first launch allow Bluetooth access. The buds must already be connected to the Mac.

## Build

```bash
bash scripts/bundle.sh   # -> target/release/Buds.app (needs Rust and Xcode 26+)
```

Protocol notes and status are in [PLAN.md](PLAN.md). Unofficial; not affiliated with OnePlus or Oppo.

## License

[PolyForm Noncommercial 1.0.0](LICENSE): free to use, modify and share (pull requests welcome) for any
noncommercial purpose; selling it or using it commercially is not allowed.
