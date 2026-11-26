# Without real-time scheduling the latency tail is not dependable under load

With every CPU busy and no real-time priority, the daemon had a run with p99 near 1 ms in 3 of 11 runs, against 1 of 11 for the input loop with no zenoh in the process. Real-time scheduling removes it, but a stock machine's limit refuses it.

**Risk.** Medium: it matters most on handhelds, where the machine is saturated and the limit is whatever the distribution set.

**Mitigation.** The daemon reports whether it was granted, and the packages raise the limit where they can.

**Closes when.** The limit is raised by every package, or the cause of the extra bad runs is found and removed.
