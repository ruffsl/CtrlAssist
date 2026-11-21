# Hiding by re-bind is observed for one kind of pad only

The re-bind half of hiding was run on a DualSense over USB, whose HID driver is `playstation`. A Bluetooth pad's HID device belongs to the Bluetooth daemon and may not come back after an unbind; an Xbox pad on USB has no HID driver and is re-bound at its USB interface.

**Risk.** High: it decides whether hiding works for those pads at all.

**Mitigation.** Run `just spike-hiding-dislodge <vendor> <product>` with a DualShock 4 over Bluetooth and an Xbox pad on USB before the hiding service is built.

**Closes when.** The recipe has been run for both.
