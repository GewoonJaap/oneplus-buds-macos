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

## Capture 4 (2026-10-06): gestures and game mode, aligned with a screen recording
Frame times in the HCI log line up with the recording (log time = recording start 13:56:07 + video seconds). Our byte order (lo, hi).

- Noise Off from the Android app is `04 04` payload `01 01 01`; Transparency is `01 01 04`. This resolves the earlier Off/Transparency question: the table is consistent (our `Off=0x08` also worked on hardware).
- Game mode: set `03 04` payload `28 <01|00>`. State is read via `0d 01` (payload `0c 05 04 0b 11 13 18 06 1b 1d 1c 27 28`), reply `0d 81` = `00 <n> (switch-id value)*`, e.g. `28 01` = game mode on.
- Gestures: read `08 01` payload `02 03 01` (reply `08 81`: `00 <n> (side mode gesture action)*`, side 01=left 02=right, mode always 01 so far).
  Write `01 04` payload `01 <side> 01 <gesture> <action>`; reply ack `01 84 00`, then the read reply is pushed.
  - gesture: 01 press, 02 double press, 03 triple press, 05 swipe, hold = separate command below. (04 appears in reads with action 08 = hold cycle; 06 with 0x12 = in-call entries, not yet written.)
  - action: 00 none, 01 play/pause, 03 voice assistant, 05 previous, 06 next, 11 (0x11) game mode; swipe only: 07 volume, 0a skip track.
  - hold noise cycle: `04 04` payload `02 01 <mask>` sent twice (left+right), mask bits 01 Off, 02 Noise cancellation, 04 Transparency (07 = all three, 06 = ANC + Transparency).
- Not yet written/verified: in-call gestures (rows "when in a call"), find earbuds, dual connection, in-ear detection.
