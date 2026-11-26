# A Steam Controller's richer haptics

A Steam Controller (2026) has haptics beyond the two-motor rumble every pad is sent: a pulse on either trackpad (output report `0x81`, which the kernel's driver sends as lizard mode changes), and the reports after it, `0x82` to `0x89`, which Steam Input uses for its own effects. A setup sends the pad only a rumble level, as its own rumble report.

**Why deferred.** Nothing a game sends can carry them. A game's force feedback on a uinput virtual pad, and a virtual DualSense's output report, are rumble, lights and trigger effects; the router's levels are two motors. Only a virtual Steam Controller would be sent them, by a game or Steam that knew it for one, and it would pass them through to a real one behind it as a virtual DualSense passes its output report to a real DualSense ([that idea](virtual-steam-controller.md)).

**Would bring it in.** The virtual Steam Controller identity.
