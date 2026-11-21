# Keeping virtual pads across a daemon restart

When the daemon stops, its virtual pads go and games see a disconnect. Their descriptors could be held outside the daemon, in systemd's descriptor store or the hiding service, and picked up by the next one.

**Why deferred.** Real added complexity; a restart is rare.

**Would bring it in.** Upgrades or crashes interrupting play in practice.
