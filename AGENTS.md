# AGENTS.md

Native macOS control for OnePlus Buds Pro 3 (replaces the HeyMelody phone app for day-to-day use).
Unofficial, reverse engineered. License: PolyForm Noncommercial 1.0.0 (no selling, PRs welcome).

## Layout

- `src/protocol.rs`: framing, enums (`Mode`, `Eq`, `Spatial`, `Side`, `Gesture`, `Action`), `Builder` (frame builders), `parse`, `decode`, unit tests. No I/O.
- `src/session.rs`: handshake plus request/response helpers (`query_*`, `set_*`) over a transport `Link`.
- `src/transport.rs`, `src/cb.rs`: CoreBluetooth transport via `objc2-core-bluetooth` (BLE GATT).
- `src/bin/buds.rs`: CLI (`status`, `battery`, `set`, `eq`, `spatial`, `game`, `gestures`, `gesture`, `hold`, `raw`, `probe`, `listen`). Use it to verify a new opcode on hardware.
- `src/bin/buds-daemon.rs`: headless worker. Reads commands on stdin, prints one JSON state line per change on stdout and writes `~/Library/Application Support/Buds/state.json`. Exits when stdin closes.
- `swift/BudsApp.swift`: SwiftUI `MenuBarExtra(.window)` UI. Spawns `buds-daemon` as a child and talks to it over pipes. Single file, built with plain `swiftc` (no Xcode project).
- `swift/Localizations/<lang>.lproj/`: `Localizable.strings` (keys are the English strings) and `InfoPlist.strings`. Add a language by copying `nl.lproj` and adding the code to `CFBundleLocalizations` in `scripts/bundle.sh`.
- `assets/BudsIcon.icon`: Icon Composer source, compiled by `actool` in `scripts/bundle.sh`.
- `scripts/bundle.sh`: builds daemon + UI and assembles/ad-hoc signs `target/release/Buds.app`.
- `src/bin/buds-menubar.rs`: old Rust tray UI, superseded by the Swift UI, kept for reference only.
- `.github/workflows/release.yml`: on push to `main` builds on `macos-26` and publishes a GitHub release `v<major.minor>.<run number>` with `Buds-<version>-macos.zip`.
- `PLAN.md`: status and the opcode tables with their source.

## Build, test, run

```bash
cargo test --lib            # protocol unit tests (golden frames from real captures)
bash scripts/bundle.sh      # -> target/release/Buds.app
pkill buds-ui; pkill buds-daemon; open target/release/Buds.app
```

Needs Rust and Xcode 26+ (the UI uses the macOS 26 Liquid Glass API behind `#available`; minimum macOS 14).

## Windows

- `src/win.rs`: WinRT transport. The buds are paired as a classic device and expose no GATT to Windows, so it connects over RFCOMM (service UUID `0000079A-D102-11E1-9B23-00025B00A5A5`, same frames) and keeps GATT as a fallback. `buds discover` lists paired devices and their services. The buds must be connected to the PC (not only paired).
- `windows/`: Tauri 2 tray app, a thin client of `buds-daemon` (spawns it, forwards command lines, relays JSON state). UI is static files in `windows/ui` (no bundler). Data dir is `%LOCALAPPDATA%\Buds` (`src/paths.rs`).
- Dev build: `cargo build --bin buds-daemon`, copy `target/debug/buds-daemon.exe` to `windows/src-tauri/binaries/buds-daemon-x86_64-pc-windows-msvc.exe`, then `cd windows && npx tauri build --debug --no-bundle` -> `windows/src-tauri/target/debug/buds-tray.exe`.
- Notification rules and battery history still live in the UI layer; the plan is to move them into the daemon.

## Hardware and sandbox rules

- The agent sandbox has no Bluetooth. Anything that talks to the buds must run in the user's own terminal (`run_in_terminal`) or be tested by the user. Do not claim a Bluetooth feature works from unit tests alone.
- Only one process should talk to the buds at a time: quit `Buds.app` (`pkill buds-ui; pkill buds-daemon`) before running `buds ...` or a standalone `buds-daemon`.
- Bluetooth permission is tied to the app bundle (`local.buds.menubar`); the CLI inherits the terminal's permission.
- Verify a write by reading the value back (`buds gestures`, `buds eq`, ...); the generic ack (`status 00`) does not prove the setting changed.
- The HeyMelody phone app only re-reads some screens when they are reopened, so it may not show changes made from the Mac immediately.

## Protocol facts (verified unless marked)

Frame: `AA <varint len = tlv_len+2> 00 00 <cmd_lo> <cmd_hi> <seq> <len_lo> <len_hi> <payload>`.
Our notation is `lo hi` (bytes on the wire). The public QuickBuds doc writes `hi lo`, so its `0406` is our `06 04`.
Replies set 0x80 on the hi byte. `Event::Other(cmd, payload)` uses `cmd = hi<<8 | lo`, e.g. EQ read reply `0x810F`, gesture read reply `0x8108`, switch read reply `0x810D`, set acks `0x84xx`, pushed events `0x0204` (noise) / `0x0504` (EQ) / `0x0510` (spatial).

Transport: BLE GATT service `0000079A-D102-11E1-9B23-00025B00A5A5`, write char `0100079A...` (write without response), notify char `0200079A...`. The Android app uses classic RFCOMM with the same frames. macOS needs `retrieveConnectedPeripherals` because the buds are already connected to the system.

Handshake: hello `aa070000000101000012`, wait at least 2 s, register `aa0c00000085 <seq> 050000 b550a069`, then about 1.6 s between steps. Too little spacing means no replies.

| Feature | Command (lo hi) | Payload |
| --- | --- | --- |
| Noise query | `0c 01` | `01 01`, reply `0c 81` with `p[1]==1` |
| Noise set | `04 04` | `01 01 <b>`: Off 08 (Android sends 01, both work), Transparency 04, High 10, Medium 20, Low 40, Adaptive 80 |
| Hold-cycle mask query | `0c 01` | `02 01`, reply `0c 81` with `p[1]==2`, mask in `p[3]` |
| Battery | `06 01`, reply `06 81` | `00 <n> (dev level\|0x80 charging)*`, dev 1 L, 2 R, 3 case; the 0x80 bit is charging (confirmed live: earbuds 50% charging, case 30% not charging) |
| EQ read / set | `0f 01` / `06 04` | read empty, reply `00 <id>`; set `<id>`: 0 Balanced, 1 Bold, 2 Serenade, 3 Bass boost, 7 Dynaudio |
| Spatial read / set | `2a 01` / `22 04` | reply `00 <mode>`; set `<mode>`: 0 off, 1 fixed, 2 head tracking |
| Switch list read | `0d 01` | `0c 05 04 0b 11 13 18 06 1b 1d 1c 27 28`, reply `00 <n> (id value)*` |
| Game mode set | `03 04` | `28 <01\|00>` (switch id 0x28) |
| Gestures read | `08 01` | `02 03 01`, reply `00 <n> (side mode gesture action)*` |
| Gesture set | `01 04` | `01 <side> 01 <gesture> <action>` |
| Hold-cycle set | `04 04` | `02 01 <mask>`, the Android app sends it twice; mask bits 01 Off, 02 ANC, 04 Transparency (at least two) |

Gesture ids: 1 press, 2 double, 3 triple, 5 swipe (4 = hold entry in reads, 6 = in-call entry with action 0x12). Sides: 1 left, 2 right.
Action ids: 00 none, 01 play/pause, 03 voice assistant, 05 previous, 06 next, 11 game mode; swipe only: 07 volume, 0a skip track.

Unverified (from the public doc, still to capture): find earbuds `00 04`, dual connection `03 04 11` plus `13 04`, in-ear detection `09 01`, in-call gestures, custom EQ `18 04`, BassWave `1b 04`.

Gotchas:
- `decode` only turns a `0c 81` reply into `Event::Noise` when `payload[1] == 1`. Other sub-queries (hold mask, ANC level) share the opcode and must stay `Event::Other`.
- Early guessed opcodes returned a generic ack that looked like success while changing nothing. Always read back.
- The case entry only appears when the buds report it (buds in or near the case); otherwise the UI shows a dash and omits it from the menu bar.

## How opcodes were found

1. Public QuickBuds protocol notes (hi-first notation).
2. Android HCI snoop capture: enable "Bluetooth HCI snoop log" in developer options with the full (unfiltered) mode, use the HeyMelody app, `adb bugreport`, extract `btsnoop_hci.log`, parse ACL/RFCOMM payloads and look for `AA ... 00 00` frames. Filtered mode truncates payloads and is useless.
3. Record the phone screen (`screenrecord`) while clicking, then align video time to log time (the recording end time equals the last click) to map each UI action to a write.

Raw captures, probes and bug reports live in `capture/` and `probe/`. They contain device names, paired host names and MAC addresses and are gitignored. Never commit them or paste those values into notes, issues or commit messages.

## UI conventions

- Design reference is the AirPods settings UI and the macOS Sound menu: four-way listening-mode picker with a draggable Liquid Glass thumb (`glassEffect` on macOS 26, flat fallback below), battery rings, list rows with checkmarks, subpages with a back chevron (Earbud Controls, Settings).
- Text must always be allowed to wrap (more languages are planned): the panel root applies `.fixedSize(horizontal: false, vertical: true)` so no height is proposed to children. Do not add `.lineLimit(1)`, fixed heights on text, or `.fixedSize()` that forces a single line.
- All user-visible strings go through `L("English key")` or a literal `Text("...")`, and must exist in every `Localizable.strings`.
- Notifications use `UNUserNotificationCenter`; a delegate is required so banners show while the panel is open. If notifications are denied, the app opens the System Settings page. Debug log: `~/Library/Application Support/Buds/notif.log`.
- Battery history: the Swift app appends a sample to `~/Library/Application Support/Buds/history.jsonl` when values change or every 5 minutes. The file is a rolling 7-day window, pruned on launch and hourly. The Statistics page (chart icon next to the gear) renders it with Swift Charts.
- Settings are stored with `@AppStorage` (`notifyLow`, `lowThreshold`, `notifyConnection`, `menuBarBattery`).
- WidgetKit desktop widget is not built: it needs an app extension and an App Group, which macOS usually will not load from an ad-hoc signed app. The daemon already writes `state.json` for a future widget.

## Release and auto-update

Repo: https://github.com/GewoonJaap/oneplus-buds-macos. `scripts/bundle.sh` takes `VERSION=<x.y.z>` and `RELEASE=1` (CI passes both); local builds are `0.1.0`.
The app's `Updater` polls `releases/latest` on launch and every 6 hours, compares the tag with `CFBundleShortVersionString`, and (if auto-install is on) downloads the `.zip` asset with `URLSession` (so no quarantine flag), unzips with `ditto`, checks the bundle id, then a detached `sh` script waits for the app to quit, swaps the `.app` and reopens it. Dev builds (Info.plist key `BudsDevBuild` is true, which is the default; CI sets `RELEASE=1` so `scripts/bundle.sh` writes false) never check automatically and never self-replace; only the manual "Check Now" button works there. Settings: `autoCheckUpdates`, `autoInstallUpdates`.
Caveat: the app is ad-hoc signed, so each update has a new code identity and macOS may ask for Bluetooth and notification permission again.

Merging to `main` publishes a release. The app is ad-hoc signed, so users must run `xattr -dr com.apple.quarantine /Applications/Buds.app` once. Real signing and notarization need an Apple developer account.
