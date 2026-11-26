# A runtime crate for what the binaries share

The tray and the settings window depend on the daemon crate for three things: its `Paths`, its durable write and its logging. That brings the device crate, the hiding service's client, evdev, udev and the journal's tracing layer into their builds, none of which they use. TisVlay keeps what its services share in a runtime crate of its own.

**Why deferred.** When the tray was built the plan said that crate arrives when a third thing needs it. The window was that third thing and took the same dependency. Nothing is wrong at run time: the cost is build time, and a dependency graph that says the window can open a pad.

**Would bring it in.** A fourth user, such as a [terminal interface](tui.md); the window or the tray needing to build where the daemon's dependencies don't, as in a [Flatpak](flatpak.md) of the window alone; or the next time those three modules are changed for another reason.
