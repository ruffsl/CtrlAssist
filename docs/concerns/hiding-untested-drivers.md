# Hiding by re-bind is unobserved for pads without a HID driver

The re-bind half of hiding was run on a DualSense over USB and a DualShock 4 over Bluetooth, both under the `playstation` HID driver, and worked the same way on each. An Xbox pad on USB has no HID driver: its `xpad` driver binds to the USB interface, so the re-bind happens at a different place in sysfs and there is no hidraw node. Xbox pads over Bluetooth use yet another driver.

**Risk.** High for Xbox pads: it decides whether hiding works for them at all.

**Mitigation.** Run `just spike-hiding-dislodge <vendor> <product>` with an Xbox pad on USB, and over Bluetooth, before the hiding service is built. The recipe already takes the USB-interface path when a pad has no HID device.

**Closes when.** The recipe has been run for an Xbox pad on each transport.
