#!/bin/sh
# Post-installation script for ctrlassist.
# Loads what the udev rule needs, and enables the units.

set -e

# Pick up the new units, and the drop-in that raises the real-time limit
# of users' service managers; a manager already running keeps its old
# limit until that user logs in again.
systemctl daemon-reload || true

# The modules now, as modules-load.d will at each boot; then the rule
# that gives their nodes to the user at the seat. Guarded so image builds
# without a kernel of their own stay quiet.
modprobe -a uinput uhid 2>/dev/null || true
udevadm control --reload 2>/dev/null || true
udevadm trigger --subsystem-match=misc --sysname-match=uinput --sysname-match=uhid 2>/dev/null || true

# The hiding service starts on demand from its socket, so only the socket
# is enabled and started. On an upgrade a running service is restarted
# onto the new binary; whatever it had hidden is restored as it stops and
# hidden again by the daemon.
systemctl enable ctrlassist-hider.socket 2>/dev/null || true
systemctl start ctrlassist-hider.socket 2>/dev/null || true
systemctl try-restart ctrlassist-hider.service 2>/dev/null || true

# The daemon and the tray are each user's own services: enabled for every
# user, started at their next login. A user who doesn't want them runs
# `systemctl --user mask ctrlassist-daemon ctrlassist-tray`.
systemctl --global enable ctrlassist-daemon.service 2>/dev/null || true
systemctl --global enable ctrlassist-tray.service 2>/dev/null || true
