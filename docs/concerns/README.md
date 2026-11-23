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
- [`rest-as-the-daemon-stops.md`](rest-as-the-daemon-stops.md) — as the daemon stops with a setup running, the hiding service shows its pads again before their writers' last reports, so a DualSense's trigger effects a game set can stay on.
- [`rest-over-bluetooth.md`](rest-over-bluetooth.md) — over Bluetooth, a pad's last report may be only queued as its driver is re-bound, and dropped, so a DualSense's trigger effects a game set can stay on after its setup stops.
- [`uinput-uhid-grant.md`](uinput-uhid-grant.md) — the seat user is given uinput and uhid to make virtual pads, which reach beyond pads; a risk where several people share a machine.
