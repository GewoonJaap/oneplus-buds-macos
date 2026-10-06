# buds-ctl plan

## Gate 0 (needs real buds): confirm protocol
`cargo run --release -- listen` -> check the handshake gets replies, and which mask bytes
the buds report when you change ANC modes on the buds themselves. Everything below depends on this.

## Phase 1: core library (buds-core)
- protocol.rs (done): framing, builders, parser
- transport: btleplug now; trait so an IOBluetooth RFCOMM transport can be swapped in
- session: handshake, seq tracking, request/response matching, auto-reconnect
- unit tests with golden frames (from IPA + public notes)

## Phase 2: features, one per HeyMelody screen (each verified against IPA strings + hardware)
ANC modes/levels, battery, EQ presets, gestures, in-ear detection, game mode,
dual connection, spatial audio, find earbuds, firmware info.
Out of scope: cloud/account, AI translation, hearing test, firmware update.

## Phase 3: macOS UX
- Menu bar app (tray-icon + tao): mode switch, battery, auto-reconnect
- Global hotkeys, `buds` CLI, Shortcuts-friendly CLI
- Widget: WidgetKit is Swift-only; optional thin Swift widget calling the CLI

## Status (2026-10-06)
Verified on hardware: handshake, noise modes, battery (L/R), equalizer presets (set 0604 <id>, read 0f01, push 0405; ids 0 balanced, 1 bold, 2 serenade, 3 bass boost, 7 dynaudio), spatial audio (set 2204 <0/1/2>, read 2a01, push 1005), menu bar app (scripts/bundle.sh -> Buds.app) with all of the above.
Opcodes came from an Android HCI snoop capture (RFCOMM, same AA framing; capture/ holds raw logs, parse scripts) plus the public QuickBuds protocol doc.
Next (commands listed in the public doc, each to be verified on the buds): gestures (read 0108, write 0401), game mode (0403 06 <v>), in-ear detection (read 0109), find earbuds (0400), dual connection (0403 11), case battery check, global hotkeys.
