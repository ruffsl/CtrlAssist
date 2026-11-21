# A pad's identifier is unverified across transports

The kernel's `uniq` value was the pad's own address on a DualSense over USB and a DualShock 4 over Bluetooth, and the PlayStation driver sets it from the pad's pairing report, so it should match across USB and Bluetooth. That was read in kernel source, not observed. The DualShock 4's value was unchanged after it slept and reconnected over Bluetooth. An Xbox One pad on USB reports an empty `uniq` and a USB serial number through udev instead; what it reports over Bluetooth, and whether that can be tied to the same pad, is unknown.

**Risk.** Medium: a pad whose address changes or is empty falls back to model and port, and a profile naming it by address stops matching.

**Mitigation.** Run `ctrlassist-hider check <node>`, which prints the identity the service reads, for the DualSense over Bluetooth and after a replug, and for an Xbox pad on USB and Bluetooth.

**Closes when.** The identifiers are recorded for those cases.
