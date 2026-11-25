# Concerns

Things the design depends on that have not been observed, or risks accepted for now. One file each: what the concern is, how much it matters, what is being done, and what would close it.

## Open

- [`hiding-untested-drivers.md`](hiding-untested-drivers.md) — hiding by re-bind is unobserved for an Xbox pad over Bluetooth.
- [`pad-marked-as-keyboard.md`](pad-marked-as-keyboard.md) — a pad with a keyboard's key on its own node, as a Stadia pad's volume keys or an Elite Series 2's keyboard collection over Bluetooth, is grabbed, not hidden; a share button, above the keyboard's keys, is hidden with its pad.
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
- [`hider-closer-on-older-kernels.md`](hider-closer-on-older-kernels.md) — before Linux 6.16, or before the service first starts with systemd before 258, a user with no seat can stop the hiding service accepting with 74 connections, one carrying a socket whose close lingers.
- [`hotplug-look-after-overrun.md`](hotplug-look-after-overrun.md) — after udev's events overrun the daemon's watch, a look at what is connected that fails is tried again only at udev's next event, the pads perhaps published wrong until then.
- [`motion-unplayed.md`](motion-unplayed.md) — a virtual DualSense's motion was seen in Steam's controller test and never in a game, through SDL or Proton, the assist's button among it.
- [`motion-untried-transports.md`](motion-untried-transports.md) — a DualSense's motion over Bluetooth and a DualShock 4's over USB are read from reports laid out by hand, never from the pad.
