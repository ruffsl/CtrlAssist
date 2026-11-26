# A joystick node the compositor also reads, hidden late, is not let go

A pad plugged in while its twin is hidden has its nodes hidden by a change event, after libinput has taken them up. For a touchpad's node the rule has udev send a remove and an add, so libinput lets it go (see [the rule and its markers](../architecture.md#the-rule-and-its-markers)). It leaves joystick nodes out, since libinput passes over a joystick.

libinput's source has it pass over a node marked a joystick only where the node has fewer than ten keys and fewer than four of a keyboard's. So a pad whose device has a plain joystick node, which is what makes it a pad to the hiding service, and a second joystick node with many keys or a touchpad's mark, would have that second node still read by the compositor after it is hidden late.

**What is known.** No such pad is known. The reading of libinput is from its source, not from a pad.

**Risk.** Low. It takes that kind of pad and a twin of it already hidden. The cost is its keys or pointer still reaching the desktop while it plays in a setup, until it is plugged in again with nothing hidden.

**Mitigation.** None.

**Closes when.** Such a pad turns up and the rule's remove and add are widened to it, or the rule is widened to every node it hides late and the virtual machine check shows no pad is the worse for it.
