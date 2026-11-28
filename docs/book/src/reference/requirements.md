# Requirements

| | |
|---|---|
| System | Linux with systemd and its udev, systemd 248 or newer, x86_64 or arm64. |
| C library | glibc 2.34 or newer for the packages: Debian 12, Ubuntu 22.04, current Fedora and Arch. NixOS builds its own. |
| Kernel | Linux 4.13 or newer, with the `uinput` module, for virtual controllers, and `uhid` and `hid-playstation`, for a virtual DualSense. Mainstream kernels ship all three, and the packages and the NixOS module load them. A Steam Controller (2026), a real one or a virtual one, needs Linux 7.3 or newer, the first release whose `hid-steam` driver knows it. |
| Desktop | Any, for the daemon and the command. The tray icon needs a StatusNotifier host: KDE Plasma has one; GNOME needs the AppIndicator extension. The GUI needs a Wayland session: it is built for Wayland only, and doesn't open in an X11 session. |
| Controllers | Anything the kernel presents as a gamepad. Xbox and PlayStation controllers are read through their own drivers, a [Steam Controller](../guide/steam-controller.md) through its own raw interface, and others through the community controller database. |

## What has been tried

CtrlAssist is developed and tested on NixOS with KDE Plasma. The least versions of systemd and Linux above were worked out from when each feature CtrlAssist uses arrived; it has been tested on systemd 262 only. The packages for Debian, Ubuntu, Fedora and Arch are installed and tried in clean containers of each, which shows they install and run there, but not that every desktop on each behaves the same. The Steam Controller has been tried only against a stand-in, never a real one ([Steam Controller](../guide/steam-controller.md)). Reports from other setups are welcome.

## Compatibility between releases

The daemon, the tray, the GUI and the command must be from the same release line: the same first two numbers, so `0.5.1` works with `0.5.3` and not with `0.6.0`. Installing a package upgrades them all together; a daemon still running from before the upgrade needs a restart.

Profiles carry a schema version. A release reads profiles written for its own schema and older ones.

This is a clean rewrite of CtrlAssist. Configuration from 0.4 and earlier is not read.
