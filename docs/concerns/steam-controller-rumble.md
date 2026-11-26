# A Steam Controller's rumble is sent as SDL sends it, unfelt

A Steam Controller (2026) pad in a setup is sent its rumble level as its own output report, `0x80`, on its raw node, the strong motor its left and the weak its right, and sent it again every 40 ms while it isn't at rest, since the pad stops its motors by itself some 50 ms after the last report, by SDL's reading. A pad no setup holds is rumbled, for `identify`, through its kernel driver's force feedback, which exists only on a kernel built with `CONFIG_STEAM_FF`.

**What only the pad can say:**

- **The gains.** The report carries a gain for each motor. SDL sends 0 for both, and the daemon follows it; the kernel's driver sends 2 and 0, from code it shares with the Steam Deck. Valve's header calls the field a signed gain in decibels. Whether either is felt as weaker, or lopsided, is unknown.
- **That 40 ms is soon enough.** A pad that stops sooner after the last report than SDL says would pulse through a long rumble.
- **That the motors stop when the level falls to rest,** and as the pad is let go, which is written as a report at rest.

**What is known.** Some kernels, nixpkgs' among them, which the development host runs, are built without `CONFIG_STEAM_FF`: there a pad no setup holds has no rumble at all, and is published as such, so that Identify is offered for it only while a profile uses it. The daemon's report and its resend are tested against the fake, by the bytes it receives and their spacing.

**Risk.** Low: rumble that is weaker or pulsing is still rumble, and nothing else rests on it.

**Closes when** a pad has been felt. On a cable, with a profile of the pad running, Identify it from the window for a second: the buzz is steady from start to end and stops when it should. In a game, or `fftest` on the virtual pad, a long rumble is steady, and the left motor stronger than the right for a strong rumble alone. Compare with Steam's own rumble test of the pad with no profile running, which sends what Steam sends; if Steam's is plainly stronger, try the kernel's gains. And on a kernel built with `CONFIG_STEAM_FF` (`zgrep STEAM_FF /proc/config.gz`, or the distribution's kernel config), with no profile using the pad, Identify it from the window: it buzzes through the kernel's own force feedback, which has never been tried, and the window offered the button because the pad's event node said it can rumble.
