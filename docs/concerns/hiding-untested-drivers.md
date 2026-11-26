# Hiding by re-bind is unobserved for an Xbox pad over Bluetooth

The re-bind half of hiding was run on a DualSense over USB, a DualShock 4 over Bluetooth and an Xbox One pad over USB, and worked the same way on each. An Xbox pad over Bluetooth uses a different driver again and has not been tried. Nor has any pad from another family.

**Risk.** Low to medium: two drivers and both transports behaved alike, so a third is likely to, but a pad whose driver doesn't re-bind cleanly couldn't be hidden from programs that already have it open.

**Mitigation.** `just hider-dev` and `just hide-pad <node>` hide any connected pad through the real service, and say whether its readers were cut off. The hiding service reports a pad it couldn't re-bind, and the setup says the pad may still be visible.

**Closes when.** The recipe has been run for an Xbox pad over Bluetooth.
