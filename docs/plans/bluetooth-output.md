# Whole output reports for a DualSense on Bluetooth

## Goal

A DualSense on USB behind a virtual DualSense is sent the game's whole output report: lights, trigger effects and rumble. One on Bluetooth is sent rumble through the kernel and nothing else, because its output report is another one, with a checksum, that the daemon doesn't write. The same holds for anything that presents itself as a DualSense on Bluetooth, as Moonshine's virtual pads do. This plan closes that gap, so a pad on Bluetooth gets what a pad on USB gets.

## What was found (2026-11-22)

Found while debugging Hades streamed through Moonshine to a DualSense and a DualShock 4 on a tablet, with a virtual DualSense in front of Moonshine's two pads.

- **Moonshine's pads are Bluetooth ones to the kernel:** HID bus `0005`, `054c:0ce6`, bound to the kernel's PlayStation driver, each with a raw node. The daemon takes neither for whole reports, `RawOutput::takes` wanting USB for a DualSense, so the game's lightbar colors, which the virtual pad does receive, go no further.
- **A DualShock 4 on Bluetooth is already written whole reports,** checksum and all (`dualshock4_report`). The DualSense's is the missing case, not a new kind of work.
- **Moonshine 0.16.1 relays rumble from its pads and nothing else.** Its binary links `inputtino_joypad_ps5_set_on_rumble` and no LED callback, so a color written to its pad is dropped there whatever the daemon does. Lights through Moonshine wait on Moonshine; this plan is worth doing for a real DualSense on Bluetooth all the same.
- **Rumble reaches both of Moonshine's pads.** Every level the game asked for was told to both, and pulses at the game's strengths and lengths, sent the daemon's way, were felt on both real pads. A report that the DualShock 4 stayed still in play was not reproduced. A pad's refused rumble write was discarded unseen; it is now tapped (`Tap::Refused`), to be looked for if it happens again.

## Decisions

- **The Bluetooth report is the USB one wrapped:** report `0x31`, a sequence and tag byte pair, the same common block a USB report `0x02` carries, and a CRC-32 over a `0xa2` seed byte and the report, as the kernel's driver writes it. Rumble goes in the report, as on USB, for the same reason: a report without the rumble mark ends the rumble.
- **A pad is taken for whole reports by what the kernel says it is,** as now: vendor, product and bus. Nothing is special to Moonshine.
- **No new time limit.** The sequence number counts reports; nothing is resent on a timer.

## Steps

1. `dualsense.rs`: the Bluetooth report for a DualSense (wrap, sequence, checksum), `RawOutput::takes` and `Kind::DualSense` taking the bus. Unit tests against bytes the kernel's driver produces for the same state, captured from a raw node.
2. A fake DualSense on Bluetooth for the tests (uhid, bus `0005`), and an integration test that a game's report reaches it whole, checksum valid, rumble replaced by the router's level.
3. On hardware: a real DualSense on Bluetooth behind a virtual DualSense, with a game that sets the lightbar and a trigger effect; and Moonshine's pads, for rumble still arriving once it travels in the report.
4. The architecture's feedback section and the guide's note on Bluetooth, which say such a pad gets rumble only.

## Deviations

None yet.
