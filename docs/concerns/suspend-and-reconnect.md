# Suspend, resume and a Bluetooth drop are untested

No setup has been run across a suspend or a Bluetooth pad power-cycling. The design says a setup is held and the pad is hidden again before a game can see it, which follows from udev applying rules before a node can be opened, but nobody has watched it. TisVlay found zenoh links can drop across a suspend.

**Risk.** Medium: these are ordinary events in a living room.

**Mitigation.** They are hardware checks on the step that adds hold and re-bind.

**Closes when.** A setup survives a suspend and a pad power-cycle with a game running.
