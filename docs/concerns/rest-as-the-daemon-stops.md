# A pad's last report can be cut off as the daemon stops

A pad given its raw node (a DualSense, a DualShock 4) has a writer thread of its own. When the pad is dropped, the writer writes what is left, then a last report that leaves the pad at rest: its motors stopped, a DualSense's trigger effects off, and the lights its driver keeps. Where the pad was hidden, the hiding service then shows it again by re-binding its driver, which cuts off a write not yet made. As a setup stops, fails or a change gives a pad up, the daemon waits for that pad's writer, off the reactor, before it asks the service to show the pad again (see [the control side](../architecture.md#the-control-side)).

As the daemon itself stops, it doesn't. Every request and reading to the service is hung up, and the connections the running setups' pads were hidden on close with the reactor, or at the process's end; the service shows each pad again as its connection closes. The setups themselves go with the input thread, which is stopped after the reactor: so the re-binds come before the writers start on their last reports, or race them.

**Risk.** Low. It takes the daemon stopping, by the service manager or a logout, while a setup runs, with a game having set a DualSense's trigger effect: the driver's re-bind sets the lights and stops the motors, and never touches the triggers, so a trigger a game made stiff or vibrating stays so until something else writes to the pad. A pad that was only grabbed gets no re-bind, and a pad sent its level through a kernel effect has nothing the re-bind leaves.

**Mitigation.** None yet. Stopping each running setup before the reactor ends would have the daemon's stop wait on the input thread and on each writer, and the reactor takes no pad's handle until something hands the pad back; the stop is written to wait on nothing.

**Closes when.** As the daemon stops, the pads of the setups still running are dropped and their writers waited for, each up to the kernel's five seconds a write on a pad that stalls, before the connections they were hidden on are hung up; or a re-bind is found to leave a DualSense's trigger effects off.
