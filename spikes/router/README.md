# Spike: does what one client serves reach another?

**Question.** The window needs the tray's notification switches, and the tray is to own and serve them as TisVlay's does. On the daemon's socket, what one client declares hadn't been reaching another. Does it, if the daemon is a zenoh router?

**What it does.** `cargo run` opens a hub on a Unix socket as the daemon does, a window as a connect-only peer (what every CtrlAssist client is today), and a tray that serves a liveliness token, a queryable and a cached publication. It prints what the window, and the hub, saw of the three, with the hub a peer or a router and the tray on a connect-only peer link or a client-mode one. With a router and a client-mode tray it then restarts the hub under both.

**Finding** (zenoh 1.10.2, 2026-11-21, the same on three runs): see the GUI plan's Spike section. In short, it is the tray's link that decides, not the hub's mode.

Deleted by the build step that gives the tray its settings service.
