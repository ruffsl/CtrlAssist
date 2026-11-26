# More virtual pad identities

A DualShock 4, a Bluetooth-style DualSense, Nintendo pads (which SDL may also read through hidraw). Legacy issue #5.

**Why deferred.** Each uhid identity is its own emulator.

**Would bring it in.** A pad owner who wants one and can test it.

**A DualShock 4 identity** would be a second emulator about the size of the DualSense one: its own input and output reports, its own feature reports and rumble layout, bound by the same kernel driver. Offered to the maintainer on 2026-11-21 and declined for now: a DualSense identity already gives PlayStation glyphs, and a DualShock 4 one matters mostly to older games that know only that pad.
