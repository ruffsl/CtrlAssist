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

# The hiding service's user and group, before anything names them: the
# hiding rule gives the group pads' files, and udev drops an assignment
# that names a group that doesn't exist. Without them the service can't
# run and the rule hides nothing, so a failure here fails the install,
# as TisVlay's does when it can't make its user.
if ! systemd-sysusers /usr/lib/sysusers.d/ctrlassist.conf; then
    echo "ctrlassist: systemd-sysusers couldn't make the user and group ctrlassist-hider." >&2
    echo "The hiding service can't run without them. Once the cause is fixed, reinstall" >&2
    echo "the package, or on Debian and Ubuntu run: dpkg --configure ctrlassist" >&2
    exit 1
fi

# The modules now, as modules-load.d will at each boot; then the rules:
# the one that gives their nodes to the user at the seat, and the hiding
# service's, applied to pads already connected. Guarded so image builds
# without a kernel of their own stay quiet.
modprobe -a uinput uhid hid-playstation hid-steam 2>/dev/null || true
udevadm control --reload 2>/dev/null || true
udevadm trigger --subsystem-match=misc --sysname-match=uinput --sysname-match=uhid 2>/dev/null || true
udevadm trigger --action=change --subsystem-match=input --property-match=ID_INPUT_JOYSTICK=1 2>/dev/null || true
udevadm trigger --action=change --subsystem-match=hidraw 2>/dev/null || true

# The hiding service starts on demand from its socket, so only the socket
# is enabled and started. On an upgrade a running service is restarted
# onto the new binary, its socket listening throughout: whatever it had
# hidden is restored as it stops, and the daemon's next connection waits
# on the socket for the new service, which hides it again. With no
# service running nothing is hidden, and the socket itself is restarted,
# as a listener keeps the settings it was made with until then. A socket
# kept listening keeps them until it next restarts, at the latest at the
# next boot; the service sees to the one that matters most, refusing
# descriptors, itself as it starts.
systemctl enable ctrlassist-hider.socket 2>/dev/null || true
if systemctl is-active --quiet ctrlassist-hider.service 2>/dev/null; then
    systemctl try-restart ctrlassist-hider.service 2>/dev/null || true
else
    systemctl try-restart ctrlassist-hider.socket 2>/dev/null || true
fi
systemctl start ctrlassist-hider.socket 2>/dev/null || true

# The daemon and the tray are each user's own services: enabled for every
# user, started at their next login. A user who doesn't want them runs
# `systemctl --user mask ctrlassist-daemon ctrlassist-tray`.
systemctl --global enable ctrlassist-daemon.service 2>/dev/null || true
systemctl --global enable ctrlassist-tray.service 2>/dev/null || true

# The settings window's entry in the launcher. Redundant where the package
# manager's own triggers do this, harmless where they don't.
update-desktop-database /usr/share/applications 2>/dev/null || true
