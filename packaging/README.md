# Packaging

How CtrlAssist is put on a machine. After TisVlay's packaging, which has the longer account of why the pipeline is shaped this way.

| Channel | Where | What it is |
|---|---|---|
| NixOS | [`nix/package.nix`](nix/package.nix), [`nix/module.nix`](nix/module.nix) | The flake's `packages.default`, `overlays.default` and `nixosModules.default`. `services.ctrlassist.enable = true` is the whole installation. [`nix/README.md`](nix/README.md) has how it builds. |
| deb, rpm, Arch | `[package.metadata.nfpm]` in [`crates/ctrlassist-cli/Cargo.toml`](../crates/ctrlassist-cli/Cargo.toml) | One package per format from the same list of contents, by `just package-deb`, `package-rpm`, `package-archlinux` or `package-all`. |

## What a package installs

- `/usr/bin/ctrlassist`, which is the command, the daemon (`service daemon`) and the tray (`service tray`), and `/usr/bin/ctrlassist-hider`, the hiding service.
- `/usr/bin/ctrlassist-gui`, the settings window, with its entry in the launcher ([`freedesktop/`](freedesktop/)) and a session bus service file by which the bus starts it: a second launch, and the tray's Settings, raise the window that is open. The Nix package writes the entry with nixpkgs' helper, so the two are kept the same by hand.
- The hiding service's socket and service as system units; the daemon and the tray as user units, enabled for every user and started at their next login.
- The hiding service's own user and group, `ctrlassist-hider`, from a `sysusers.d` file ([`sysusers/`](sysusers/)); the NixOS module declares them itself. The service runs as that user with no capabilities.
- A udev rule that gives uinput and uhid to the user at the seat, and a `modules-load.d` file that loads both, and the kernel's PlayStation driver, which a virtual DualSense is a pad of.
- The hiding service's udev rule, `72-ctrlassist-hide.rules`, which is all the power the service has: it hides the nodes of pads named by marker files in the service's runtime directory, gives the service's group the `uevent` file of each pad's HID device or USB interface and re-binds a pad's driver when the service writes a marked event there, and gives the group PlayStation pads' raw nodes. It runs `chgrp`, `chmod` and `udevadm` from `/usr/bin`, which the Nix package points at the store. The [architecture](../docs/architecture.md#hiding) has why.
- A drop-in on `user@.service` that lets users' services ask for real-time scheduling up to priority 10, which the daemon's input thread does. It reaches no login shell. Removing the file turns it off; [Real-time scheduling](#real-time-scheduling) has what was measured and why this way.
- Completions for bash, zsh and fish.
- `NOTICE`, which carries the bundled controller database's license, and `THIRD-PARTY-LICENSES.html`, the license of every crate built in, under `/usr/share/doc/ctrlassist/`.

The scripts in [`scripts/`](scripts/) run at install, upgrade and removal:

- `postinstall.sh` makes the hiding service's user with `systemd-sysusers`, loads the modules, reloads udev and systemd, applies the rules to pads already connected, enables the units, and on an upgrade restarts a running hiding service onto the new binary, its socket listening throughout, or with none running restarts the socket, whose listener keeps the settings it was made with until then. deb and rpm run it on an install and on every upgrade; the Arch package runs it as `post_install` and as `post_upgrade`, since pacman runs only the second on an upgrade. If the user can't be made, it says so and fails the install, as TisVlay's does: the hiding service can't run without its user, and udev drops what the rule assigns to a group that doesn't exist.
- `preremove.sh`, on a removal and never on an upgrade (an rpm count of 1 or more), disables the units and stops the hiding service, which restores whatever it had hidden, and waits for udev to finish the re-binds that asks for, before the rule that does them is deleted.
- `postremove.sh`, once the files are gone, has systemd and udev read what is left, so the units and the real-time drop-in are forgotten and the rule's grant of uinput and uhid is no longer applied. Whether a grant already made is taken back before the next boot is udev's and logind's to decide. PlayStation pads' raw nodes go back to root alone. The hiding service's user stays, as system users do when their package goes.

Users' running daemons, trays and windows are left running, as TisVlay leaves its user services: a package script has no safe hold on a live user manager, and stopping a daemon would take its virtual pads from under a game. They run from the removed binaries until each user logs out; `preremove.sh` says so, with the command to stop them sooner.

## How the binaries are built

`just package-build` builds with the `dist` profile through cargo-zigbuild, with zig as the linker and `x86_64-unknown-linux-gnu.2.34` as the target. The binaries then name the standard ELF interpreter, not a Nix store path, and use no glibc symbol newer than 2.34, so they run on Debian 12, Ubuntu 22.04, Fedora and Arch. They link libudev, and the hiding service libsystemd, by their plain sonames, which each package declares as a dependency, with systemd and udev themselves at 248 or newer. The window links fontconfig too, so the loader needs it before the window's first line runs, and loads OpenGL, EGL, the Wayland client libraries and libxkbcommon at run time, which no linker lists; each package declares all of them, and the Nix package puts them on the window's rpath.

## Real-time scheduling

The daemon runs as a systemd user service and its input thread asks for first-in-first-out scheduling at priority 10, which a stock machine refuses. Three ways of allowing it were measured on 2026-11-21, each on a NixOS virtual machine of its own built from the pinned nixpkgs:

| Machine | In a user service | In a login shell | The user's service manager |
|---|---|---|---|
| Stock | limit 0, refused | limit 0 | limit 0 |
| Drop-in: `LimitRTPRIO=10` on `user@.service` | limit 10, granted | limit 0 | limit 10 |
| PAM: `security.pam.loginLimits`, rtprio 10 for the user | limit 10, granted | limit 10 | limit 10 |

- **The drop-in and a PAM limit both work for a user service.** They differ in reach: the drop-in raises the limit for every user's services and nothing else, while a PAM limit raises it for the users named, for everything they run, shells included. Elsewhere than NixOS a PAM limit reaches the user manager only where the distribution's PAM setup for it calls `pam_limits`: systemd's own `systemd-user` fragment hasn't since version 232, though Debian adds it directly and Fedora and Arch through the `system-auth` they include (read from their packaging, not measured).
- **`LimitRTPRIO=` in the daemon's own unit changes nothing,** on any machine: a user's service manager is unprivileged, and `systemd.exec(5)` says such a setting in a user service can't raise a limit above the manager's; systemd lowers the request to the manager's hard limit without a word (`setrlimit_closest`). So the line can cap, never grant, and the daemon's unit doesn't carry it.
- **rtkit is no alternative here.** It grants only to a user with a login session, so not to a daemon started by lingering; only with an `RLIMIT_RTTIME` of 200 ms or less set first; and only round-robin. A thread that then runs 200 ms without blocking gets the whole process killed, which for the daemon is every virtual pad. It is the one of the three available inside a Flatpak, through the portal.

The drop-in is the narrowest that works, user services only, priority 10 only, with no group to join and no CPU-time limit, and is what openSUSE ships for PipeWire. Not tried: Debian, Fedora or Arch themselves, a Flatpak, the portal. The spike that measured this, a NixOS test of five machines, is in the history at `cf28e7a` under `spikes/realtime/`.

## Trying a package

`just package-test` installs each package with its distribution's own package manager in a clean container (Debian 12, Ubuntu 22.04 as the oldest glibc covered, Fedora, Arch) and checks that the command runs there, reports no daemon, completes its own words, that the hiding service runs and refuses what isn't a pad, and that every shipped file is in place. It needs docker. The containers have no systemd, so the units are checked for being there, not for starting; the NixOS VM check `daemon` starts them.

## Licenses

Two files in [`licensing/`](licensing/) say what may be depended on, and two say what was:

- `deny.toml` is the policy: `just deny` fails on a license outside its list, a crate in two versions, or a source that isn't crates.io.
- `about.toml` and `about.html.hbs` drive cargo-about, which writes `THIRD-PARTY-LICENSES.html` for the packages (`just licenses-html`, and the Nix package's own build). It fails on a crate whose license `about.toml` doesn't accept, so the two lists are kept the same; a dependency that brings a new license fails both until it is looked at.
- `NOTICE` is the project's own, and carries the license of the one thing bundled that isn't a crate: the controller database snapshot.

The bundle is generated, not committed.

## Releasing

Workflows after TisVlay's, without its crates.io and Nix cache phases: nothing here is published to crates.io.

1. **Bump.** `just release-bump` runs release-plz, which picks the version from the commit history and writes the CHANGELOGs; merge that as a `chore(release)` change. `release-plz.yml` opens the same change as a pull request on each push to `main`.
2. **Tag.** Pushing `vX.Y.Z` runs `release.yml`. It calls `build-packages.yml`, which builds the three packages and one SBOM per binary for x86_64 and arm64, checks each SBOM against what its binary embeds, and signs them through Sigstore: a provenance attestation over the packages and binaries, and an SBOM attestation per binary. Building and signing are separate jobs, so the job that runs every dependency's build scripts holds no signing token.
3. **Verify.** The workflow then checks every signature as a user would (`just verify-assets`), and only then attaches everything, with a checksum file, to a draft release.
4. **Publish.** `just verify-release vX.Y.Z` repeats the check on the draft from your own machine. Test the packages, then publish the draft by hand.

`audit.yml` checks the dependencies against the RustSec advisory database weekly and whenever the lockfile changes (`just audit`). `pr-title.yml` holds pull request titles, and the commits of a plan-driven one, to Conventional Commits, which is what release-plz reads. `test-report.yml` turns CI's test results into a check run.

None of this has run: a real run needs the repository in the `CtrlAssist` organization, which the signer's identity names. The workflows pass `actionlint` and `zizmor`, and `just sbom-check` passes locally.
