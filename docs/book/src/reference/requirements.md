# Requirements

| | |
|---|---|
| System | Linux with systemd, x86_64 or arm64. |
| C library | glibc 2.34 or newer for the packages: Debian 12, Ubuntu 22.04, current Fedora and Arch. NixOS builds its own. |
| Kernel | The `uinput` module, which every mainstream kernel ships; the packages load it. |
| Desktop | Any, for the daemon and the command. The tray icon needs a StatusNotifier host: KDE Plasma has one; GNOME needs the AppIndicator extension. |
| Controllers | Anything the kernel presents as a gamepad. Xbox and PlayStation controllers are read through their own drivers; others through the community controller database. |

## What has been tried

CtrlAssist is developed and tested on NixOS with KDE Plasma. The packages for Debian, Ubuntu, Fedora and Arch are installed and exercised in clean containers of each as part of building them, which shows they install and run there, but not that every desktop on each behaves the same. Reports from other setups are welcome.

## Compatibility between releases

The daemon, the tray and the command must be from the same release line: the same first two numbers, so `0.5.1` works with `0.5.3` and not with `0.6.0`. Installing a package upgrades all three together; a daemon still running from before the upgrade needs a restart.

Profiles carry a schema version. A release reads profiles written for its own schema and older ones.

This is a clean rewrite of CtrlAssist. Configuration from 0.4 and earlier is not read.
