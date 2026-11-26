# A `watch` verb

TisVlay's CLI has a verb that stays running and prints each change of state as it happens. CtrlAssist's has `status`, `pads` and `setups`, each of which prints once and ends; the SDK's state watcher, which the tray and the window are built on, would serve a `watch` as it is.

**Why deferred.** The tray's notifications and the window already show changes as they happen, and no script has wanted them.

**Would bring it in.** Someone scripting on a pad connecting or a setup starting, or a bug that needs the order of changes seen from outside the daemon's log.
