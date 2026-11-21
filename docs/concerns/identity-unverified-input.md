# Virtual pad identities were detected, not played

Each identity was listed and recognized by SDL and Steam, and rumble reached it. No buttons were pressed against a game, so whether each mapped control lands where it should, and what glyphs a game shows, are unverified. Gyro, touchpad and LEDs through a uhid pad, a Bluetooth-style DualSense, a DualShock 4 identity and any Proton game are untested.

**Risk.** Medium: a wrong mapping is obvious the first time someone plays, and cheap to fix.

**Mitigation.** Each identity's build step ends with a named hardware check in a game.

**Closes when.** Each shipped identity has been played in a native game and a Proton game.
