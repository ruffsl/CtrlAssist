# A touchpad shown again without a re-bind stays let go by the compositor

The rule sets `LIBINPUT_IGNORE_DEVICE=1` on each event node of a hidden pad, and libinput reads it only as it adds a node. A touchpad hidden by a change event rather than by a re-bind, as one of a pad plugged in while hidden is, was added unhidden, so the rule has udev tell its listeners the node went and came back, and libinput lets it go (see [the rule and its markers](../architecture.md#the-rule-and-its-markers)). Showing the pad again re-binds its driver, which makes every node anew, and libinput takes the touchpad up again as it is added. A touchpad whose markers go with no re-bind after is shown at its next event instead, a change, which libinput ignores: the node is open to the seat again, but libinput, having let it go, doesn't take it up until it is next added.

That happens in two ways, neither seen:

- **A hide whose re-bind was never asked,** udev not being listened to or the request not written, is shown again without one, its markers deleted; if a change event reached the touchpad, and not the pad's joystick node, in the moment its markers were there, it was let go then, and the sweep after, below, doesn't find it.
- **A device whose touchpad alone is hidden,** by a change event of that node alone while a marker made for another pad, a twin with its address, named it. Once that marker goes, the sweep finds what the rule hides that nothing should by a pad's joystick node or its raw node, not by a touchpad's, so it asks no re-bind for that device.

**Risk.** Low. Each takes a failure or a change event on one touchpad node alone, at the moment a marker names it. The cost is a pad's touchpad that doesn't move the pointer until the pad is plugged in again; the pad itself, and a game reading its touchpad, are as they would be.

**Mitigation.** None beyond plugging the pad in again.

**Closes when.** The rule, on a change event that leaves a touchpad's node unhidden where udev's record of its last event says the rule hid it (`CTRLASSIST_HIDDEN`, imported from that record), has udev send a remove and an add, as it does when it hides one late; and the sweep looks for any event node the rule hid, a touchpad's among them, not only a pad's. Neither is done: the first adds work to every change event of a pad's touchpad, and the virtual machine check would need a way to make either case to show it.
