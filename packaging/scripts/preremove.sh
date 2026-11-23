#!/bin/sh
# Pre-removal script for ctrlassist.
# Stops and disables its units on a real removal, never on an upgrade.

set -e

# The three packagers sharing this script say removal or upgrade in $1
# differently, so the upgrade signals are excluded rather than removals
# listed:
#   rpm    %preun      a count: 0 = removal, 1 or more = upgrade
#   deb    prerm       a word: remove = removal; upgrade, failed-upgrade
#                      and deconfigure are not
#   pacman pre_remove  the version, and pacman runs this hook only on
#                      removal, so a version falls through to teardown
case "$1" in
1 | upgrade | failed-upgrade | deconfigure)
    # An upgrade: the new package's postinstall sees to the units.
    ;;
*)
    systemctl --global disable ctrlassist-daemon.service 2>/dev/null || true
    systemctl --global disable ctrlassist-tray.service 2>/dev/null || true
    # Stopping the service restores whatever it had hidden.
    systemctl stop ctrlassist-hider.socket ctrlassist-hider.service 2>/dev/null || true
    systemctl disable ctrlassist-hider.socket 2>/dev/null || true
    # Users' running daemons, trays and windows are left alone, as
    # TisVlay's are: a package script has no safe hold on a live user
    # manager, and stopping a daemon mid-game would take its virtual pads
    # from under the game. They keep running from the removed binaries
    # until each user logs out; postremove has udev and systemd forget
    # the rest.
    echo "ctrlassist removed. Each user's daemon and tray keep running until they log out;"
    echo "to stop yours now: systemctl --user stop ctrlassist-daemon ctrlassist-tray"
    ;;
esac
