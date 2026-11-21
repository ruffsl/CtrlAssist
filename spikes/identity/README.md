# Spike: virtual pad identity

Throwaway code for the plan's identity spike. It answers: what does it take for a virtual pad to pass as a specific pad, a DualSense above all?

`src/main.rs` builds `ident`:

- `capture <hidraw>` prints what a connected DualSense gives an emulator to replay.
- `uinput-pad` creates an evdev-level pad with any name and IDs and logs the force feedback it is sent.
- `uhid-dualsense <hidraw>` creates a HID-level DualSense from a connected one's report descriptor and feature reports, and logs the output reports it is sent.

The results are in `docs/architecture.md`, under "What the platform does". This directory is deleted when the device crate replaces it.
