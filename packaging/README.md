# Packaging

How CtrlAssist is put on a machine. After TisVlay's packaging, which has the longer account of why the pipeline is shaped this way.

| Channel | Where | What it is |
|---|---|---|
| NixOS | [`nix/package.nix`](nix/package.nix), [`nix/module.nix`](nix/module.nix) | The flake's `packages.default`, `overlays.default` and `nixosModules.default`. `services.ctrlassist.enable = true` is the whole installation. |
| deb, rpm, Arch | `[package.metadata.nfpm]` in [`crates/ctrlassist-cli/Cargo.toml`](../crates/ctrlassist-cli/Cargo.toml) | One package per format from the same list of contents, by `just package-deb`, `package-rpm`, `package-archlinux` or `package-all`. |

## What a package installs

- `/usr/bin/ctrlassist`, which is the command, the daemon (`service daemon`) and the tray (`service tray`), and `/usr/bin/ctrlassist-hider`, the hiding service.
- The hiding service's socket and service as system units; the daemon and the tray as user units, enabled for every user and started at their next login.
- A udev rule that gives uinput and uhid to the user at the seat, and a `modules-load.d` file that loads both.
- A drop-in on `user@.service` that lets users' services ask for real-time scheduling up to priority 10, which the daemon's input thread does. It reaches no login shell. Removing the file turns it off; `spikes/realtime/README.md` has what was measured and why this way.
- Completions for bash, zsh and fish.
- `NOTICE`, which carries the bundled controller database's license, and `THIRD-PARTY-LICENSES.html`, the license of every crate built in, under `/usr/share/doc/ctrlassist/`.

The scripts in [`scripts/`](scripts/) run at install and removal: they load the modules, reload udev, and enable the units, and on removal undo that. They do nothing on an upgrade but restart the hiding service onto the new binary.

## How the binaries are built

`just package-build` builds with the `dist` profile through cargo-zigbuild, with zig as the linker and `x86_64-unknown-linux-gnu.2.34` as the target. The binaries then name the standard ELF interpreter, not a Nix store path, and use no glibc symbol newer than 2.34, so they run on Debian 12, Ubuntu 22.04, Fedora and Arch. They link libudev, and the hiding service libsystemd, by their plain sonames, which each package declares as a dependency.

## Trying a package

`just package-test` installs each package with its distribution's own package manager in a clean container (Debian 12, Ubuntu 22.04 as the oldest glibc covered, Fedora, Arch) and checks that the command runs there, reports no daemon, completes its own words, that the hiding service runs and refuses what isn't a pad, and that every shipped file is in place. It needs docker. The containers have no systemd, so the units are checked for being there, not for starting; the NixOS VM check `daemon` starts them.

## Licenses

Two files in [`licensing/`](licensing/) say what may be depended on, and two say what was:

- `deny.toml` is the policy: `just deny` fails on a license outside its list, a crate in two versions, or a source that isn't crates.io.
- `about.toml` and `about.html.hbs` drive cargo-about, which writes `THIRD-PARTY-LICENSES.html` for the packages (`just licenses-html`, and the Nix package's own build). It fails on a crate whose license `about.toml` doesn't accept, so the two lists are kept the same; a dependency that brings a new license fails both until it is looked at.
- `NOTICE` is the project's own, and carries the license of the one thing bundled that isn't a crate: the controller database snapshot.

The bundle is generated, not committed.
