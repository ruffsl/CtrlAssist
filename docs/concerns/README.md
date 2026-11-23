# Concerns

Things the design depends on that have not been observed, or risks accepted for now. One file each: what the concern is, how much it matters, what is being done, and what would close it.

## Open

- [`hiding-untested-drivers.md`](hiding-untested-drivers.md) — hiding by re-bind is unobserved for an Xbox pad over Bluetooth.
- [`pad-marked-as-keyboard.md`](pad-marked-as-keyboard.md) — a pad udev also marks as having keyboard keys, as an Xbox Series pad's Share button may, is grabbed, not hidden.
- [`pad-identity-across-transports.md`](pad-identity-across-transports.md) — a pad's identifier is unverified across transports.
- [`suspend-and-reconnect.md`](suspend-and-reconnect.md) — suspend, resume and a Bluetooth drop are untested.
- [`realtime-refused.md`](realtime-refused.md) — without real-time scheduling the latency tail is not dependable under load.
- [`identity-unverified-input.md`](identity-unverified-input.md) — virtual pad identities were detected, not played.
- [`lz4-flex-advisory.md`](lz4-flex-advisory.md) — zenoh pins an lz4_flex with a decompression advisory, unreachable while compression is off.
- [`slint-unstable-winit-030.md`](slint-unstable-winit-030.md) — the window reaches winit through a Slint feature outside its stability promise, so Slint is pinned exactly. After TisVlay's.
- [`slint-combobox-popup-clipped.md`](slint-combobox-popup-clipped.md) — Slint's stock dropdown clips at the window's edge; the window uses TisVlay's flip-up `ThemedSelect` until native popups land.
- [`final-report-after-drop.md`](final-report-after-drop.md) — a pad's writer thread isn't waited for when its setup stops, so its last report, the one that turns off a DualSense's trigger effects, can lose a race with a re-bind; the device crate gives a handle to wait on, which the daemon doesn't use yet.
- [`uinput-uhid-grant.md`](uinput-uhid-grant.md) — the seat user is given uinput and uhid to make virtual pads, which reach beyond pads; a risk where several people share a machine.
