# Concerns

Things the design depends on that have not been observed, or risks accepted for now. One file each: what the concern is, how much it matters, what is being done, and what would close it.

## Open

- [`hiding-untested-drivers.md`](hiding-untested-drivers.md) — hiding by re-bind is unobserved for an Xbox pad over Bluetooth.
- [`pad-marked-as-keyboard.md`](pad-marked-as-keyboard.md) — a pad with a keyboard's key on its own node, as a Stadia pad's volume keys or an Elite Series 2's keyboard collection over Bluetooth, is grabbed, not hidden; a share button, above the keyboard's keys, is hidden with its pad.
- [`pad-identity-across-transports.md`](pad-identity-across-transports.md) — a pad's identifier is unverified across transports.
- [`suspend-and-reconnect.md`](suspend-and-reconnect.md) — suspend, resume and a Bluetooth drop are untested.
- [`realtime-refused.md`](realtime-refused.md) — without real-time scheduling the latency tail is not dependable under load.
- [`slide-text-right-to-left.md`](slide-text-right-to-left.md) — a name too long for its place slides the wrong way in a right-to-left script.
- [`identity-unverified-input.md`](identity-unverified-input.md) — virtual pad identities were detected, not played.
- [`lz4-flex-advisory.md`](lz4-flex-advisory.md) — zenoh pins an lz4_flex with a decompression advisory, unreachable while compression is off.
- [`slint-unstable-winit-030.md`](slint-unstable-winit-030.md) — the window reaches winit through a Slint feature outside its stability promise, so Slint is pinned exactly. After TisVlay's.
- [`slint-combobox-popup-clipped.md`](slint-combobox-popup-clipped.md) — Slint's stock dropdown clips at the window's edge; the window uses TisVlay's flip-up `ThemedSelect` until native popups land.
- [`rest-as-the-daemon-stops.md`](rest-as-the-daemon-stops.md) — as the daemon stops with a setup running, the hiding service shows its pads again before their writers' last reports, so a DualSense's trigger effects a game set can stay on.
- [`rest-over-bluetooth.md`](rest-over-bluetooth.md) — over Bluetooth, a pad's last report may be only queued as its driver is re-bound, and dropped, so a DualSense's trigger effects a game set can stay on after its setup stops.
- [`uinput-uhid-grant.md`](uinput-uhid-grant.md) — the seat user is given uinput and uhid to make virtual pads, which reach beyond pads; a risk where several people share a machine.
- [`steam-raw-node-grant.md`](steam-raw-node-grant.md) — the seat user is given a Steam Controller pad's raw node, which any program of theirs can then read and write, firmware commands included, and through which SDL3 and Steam read the pad while a setup holds it; another program's hold of it reads to the daemon as the pad leaving.
- [`steam-controller-haptics.md`](steam-controller-haptics.md) — a virtual Steam Controller's haptics are passed to the pads behind it unfelt, sound for the actuators unpaced, and one report nobody knows the meaning of.
- [`steam-controller-hiding.md`](steam-controller-hiding.md) — a Steam Controller is hidden only on its cable, by an exception for its own keyboard and pointer and a marker nobody has seen name a real pad's raw node; elsewhere it is only grabbed, and Steam and SDL3's own driver read the real pad beside the virtual one.
- [`steam-controller-report.md`](steam-controller-report.md) — a Steam Controller's report is read as the kernel's driver and SDL describe it, never from a pad: the trackpads' vertical sense, an untouched trackpad, the triggers' range, report `0x47`, a repeated sensor clock, motion before it is asked for, the motion's axes.
- [`steam-controller-lizard-mode.md`](steam-controller-lizard-mode.md) — whether a Steam Controller taken out of lizard mode the kernel's way stops typing and pointing, `EPIPE` over the air, a pad slow to a feature report holding every raw node on the machine, and what Steam's own settings do to it meanwhile.
- [`steam-controller-puck-and-bluetooth.md`](steam-controller-puck-and-bluetooth.md) — a Steam Controller behind the Puck or over Bluetooth is untried: their descriptors, slots, product numbers by transport, and two pads on one adapter.
- [`steam-controller-rumble.md`](steam-controller-rumble.md) — a Steam Controller's rumble report, its gains of 0 where the kernel sends 2, and its resend every 40 ms, have never been felt; a free pad has no rumble on kernels without `CONFIG_STEAM_FF`.
- [`steam-controller-identity.md`](steam-controller-identity.md) — a Steam Controller is known by its serial as its address, unseen across transports, and the driver names a pad whose serial it couldn't ask `XXXXXXXXXX`.
- [`hider-closer-on-older-kernels.md`](hider-closer-on-older-kernels.md) — before Linux 6.16, or before the service first starts with systemd before 258, a user with no seat can stop the hiding service accepting with 74 connections, one carrying a socket whose close lingers.
- [`hotplug-look-after-overrun.md`](hotplug-look-after-overrun.md) — after udev's events overrun the daemon's watch, a look at what is connected that fails is tried again only at udev's next event, the pads perhaps published wrong until then.
- [`touchpad-shown-without-rebind.md`](touchpad-shown-without-rebind.md) — a hidden pad's touchpad that libinput let go, shown again with no re-bind after, as a hide whose re-bind was never asked is, stays let go until the pad is plugged in again.
- [`motion-unplayed.md`](motion-unplayed.md) — a virtual DualSense's motion was seen in Steam's controller test and never in a game, through SDL or Proton, the assist's button among it.
- [`motion-untried-transports.md`](motion-untried-transports.md) — a DualSense's motion over Bluetooth and a DualShock 4's over USB are read from reports laid out by hand, never from the pad.
- [`kde-tray-submenu-collapse.md`](kde-tray-submenu-collapse.md) — KDE's tray may close a profile's submenu the first time one of its fields is opened, an upstream bug TisVlay records; not looked for here.
- [`joystick-node-hidden-late.md`](joystick-node-hidden-late.md) — a joystick node with many keys or a touchpad's mark, hidden after libinput took it up, would still be read by the compositor; no such pad is known.
- [`cli-closed-pipe-untested.md`](cli-closed-pipe-untested.md) — the CLI's quiet end on a closed pipe was copied from TisVlay without the test that holds it.
- [`hide-refused-not-asked-again.md`](hide-refused-not-asked-again.md) — a hide the service refuses for a moment, as for a session not in front, is not asked again until the pad returns or its setup restarts, the pad staying grabbed.
- [`virtual-dualsense-refused-under-load.md`](virtual-dualsense-refused-under-load.md) — the kernel's driver refused a virtual DualSense once, with every core busy; the cause is a guess.
- [`browser-keeps-raw-nodes.md`](browser-keeps-raw-nodes.md) — Chrome keeps the raw nodes of pads that are gone, of the 64 the machine has: seen with the tests' pads, where it fails them, and by the tests run together alone, not singly, nor by one setup started and stopped twenty times; how many pads at once it takes is unknown.
