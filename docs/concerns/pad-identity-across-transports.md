# A pad's address is unverified for Xbox pads and across transports

The kernel's `uniq` value was the pad's own address on a DualSense over USB and a DualShock 4 over Bluetooth, and the PlayStation driver sets it from the pad's pairing report, so it should match across USB and Bluetooth. That was read in kernel source, not observed. What an Xbox pad reports is unknown.

**Risk.** Medium: a pad whose address changes or is empty falls back to model and port, and a profile naming it by address stops matching.

**Mitigation.** Run `just spike-hiding-ids "<label>"` for the DualSense over Bluetooth and after a replug, and for an Xbox pad on USB and Bluetooth.

**Closes when.** The identifiers are recorded for those cases.
