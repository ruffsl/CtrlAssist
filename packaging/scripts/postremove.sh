#!/bin/sh
# Post-removal script for ctrlassist.
# Has systemd and udev forget the files the package took away.

set -e

# Runs after the files are gone, which preremove can't see. On an upgrade
# the new package's files are already in place, so the same reloads only
# read them again: nothing here needs to tell removal from upgrade.
#   rpm    %postun      a count: 0 = removal, 1 or more = upgrade
#   deb    postrm       remove, purge, upgrade, failed-upgrade, abort-*
#   pacman post_remove  the old version, and only on removal

# The units and the drop-in on user@.service are gone. A user's service
# manager already running keeps the real-time limit it started with until
# that user logs in again.
systemctl daemon-reload 2>/dev/null || true

# The rule that gave uinput and uhid to the user at the seat is gone; ask
# udev to apply the rules as they are now to both nodes. Whether access
# granted before is taken back at once is udev's and logind's to decide;
# from the next boot it is not given. The modules stay loaded, as other
# software may use them; modules-load.d no longer loads them at boot.
udevadm control --reload 2>/dev/null || true
udevadm trigger --subsystem-match=misc --sysname-match=uinput --sysname-match=uhid 2>/dev/null || true

# The settings window's entry is gone from the launcher. Redundant where
# the package manager's own triggers do this, harmless where they don't.
update-desktop-database /usr/share/applications 2>/dev/null || true
