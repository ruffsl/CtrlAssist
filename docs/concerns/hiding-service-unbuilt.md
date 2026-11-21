# The hiding service is proven as commands, not as a process

Everything the hiding service does was done by hand with root commands: the rule, the re-bind, the restore. Passing descriptors over a socket, the seat check, the checks on what counts as a pad, cleanup when a connection closes, and reconciliation after its own crash exist only on paper.

**Risk.** High: every step after it assumes pads arrive as descriptors.

**Mitigation.** It is built early, with a virtual machine test that hides, kills, restores and reconnects, and a review of the root code before anything depends on it.

**Closes when.** The service passes its virtual machine test and a real pad check against Steam and SDL.
