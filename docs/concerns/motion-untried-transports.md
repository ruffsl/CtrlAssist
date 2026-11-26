# A DualSense's motion over Bluetooth, and a DualShock 4's over USB, are untried

A pad's motion and touchpad are read from its raw report, which differs by transport. A DualSense was read on USB and a DualShock 4 over Bluetooth, on real pads. The other two are read from reports laid out by hand, as the kernel's driver and SDL describe them: a DualSense's Bluetooth report (0x31, the same body a byte later), and a DualShock 4's USB report with its calibration under another number and in another order.

Also unseen on a pad: a DualSense over Bluetooth is said to report 1,000 times a second, four times its rate on USB, which the bench has timed with fake pads alone; whether its own clock and its reports' arrival part there; a DualShock 4's turn against a straight edge, to check its calibration as a DualSense's was; and a tap on a DualShock 4's touchpad that lands and lifts between two reports, which is read from the report's earlier touch readings.

**Risk.** Low to medium: a wrong offset would show as motion that is plainly wrong or absent on that transport, not as a subtle fault.

**Mitigation.** Each layout has unit tests from bytes laid out as the kernel's structs give them, and a report that isn't the size and number expected is passed over, the pad's motion staying at rest.

**Closes when.** A DualSense paired over Bluetooth and a DualShock 4 on a cable have each been seen to carry their motion and touchpad to a virtual DualSense in Steam's controller test.
