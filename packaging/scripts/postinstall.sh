#!/bin/sh
# Post-installation script for ctrlassist.
# Loads what the udev rule needs, and enables the units.
#
# Runs on an install and on every upgrade: deb and rpm run it for both,
# and the Arch package runs it as post_upgrade too, since pacman runs
# post_install only on a first install. Nothing in it depends on which.

set -e

# Pick up the new units, and the drop-in that raises the real-time limit
# of users' service managers; a manager already running keeps its old
# limit until that user logs in again.
systemctl daemon-reload || true

# The modules now, as modules-load.d will at each boot; then the rule
# that gives their nodes to the user at the seat. Guarded so image builds
# without a kernel of their own stay quiet.
modprobe -a uinput uhid hid-playstation 2>/dev/null || true
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

# The settings window's entry in the launcher. Redundant where the package
# manager's own triggers do this, harmless where they don't.
update-desktop-database /usr/share/applications 2>/dev/null || true
