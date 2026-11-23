# A pad's last output report can land after the pad is given back

A pad given its raw node (a DualSense, a DualShock 4) has a writer thread of its own, so a pad that has stopped answering can't hold the input thread: a write to a raw node on USB can wait five seconds in the kernel. When the pad is dropped, the writer writes what is left, the level that stops its rumble among it, and ends. Whoever dropped the pad doesn't wait for that, so the write races what comes next, and where the pad was hidden, that is the hiding service re-binding its driver.

**Risk.** Low. On a pad that answers, the write takes about a millisecond and comes first. Where it loses to a re-bind, the re-bind resets the pad, which stops a DualSense's rumble and lights on its own. A pad that was only grabbed gets no re-bind, and then nothing races the write. What is left is a pad that keeps the last rumble or lights a game set, until it is used again.

**Mitigation.** None in place. Waiting on drop was set aside: the drop happens in the daemon's reactor, so a stalled pad on USB would hold every request for up to five seconds.

**Closes when.** Giving a pad back waits for that pad's writer, off the reactor: the stop answers at once, and only that pad's restore follows its last write. To take up once the hiding service's redesign has landed, since both change the restore path.
