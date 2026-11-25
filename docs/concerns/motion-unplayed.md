# A virtual DualSense's motion was never tried in a game

A hidden DualSense's and DualShock 4's gyroscope, accelerometer and touchpad reach a virtual DualSense, and Steam's controller test shows them moving as the pad does. No game has been played with them: not one that aims by gyro, not one that steers by tilt, not through SDL natively and not under Proton, and Steam Input's own gyro mappings were not tried against the virtual pad. The assist's button that takes the motion was tried in Steam's controller test alone.

What a game could show that the test doesn't: the virtual pad's neutral calibration or its own sensor clock read otherwise than a real pad's; the step in gravity as an assist's sensors stand in and go back, in a game that steers by tilt; and a jump in drift at the same moment, the two pads' stored biases differing, in a game that has learned one pad's.

**Risk.** Medium: motion that is wrong in a game is plain at once, but only to someone who plays that way, and the hand-over's faults would look like the game's.

**Mitigation.** The readings are checked against the kernel's driver's and SDL's own arithmetic in the device crate's tests, and end to end in the daemon's VM check.

**Closes when.** A game that aims by gyro and one that steers by tilt have each been played through a virtual DualSense, natively and under Proton, with the assist taking the motion and handing it back. A step or a jump that shows at the hand-over is a reason to look again at a ramp there, or at [bias measured at rest](../ideas/gyro-bias-at-rest.md).
