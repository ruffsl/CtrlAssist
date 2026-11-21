# Installation

CtrlAssist runs on Linux. It installs one command, `ctrlassist`, which is also the daemon and the tray, and a small system service that hides controllers from games.

## NixOS

Add the flake and enable the module:

```nix
{
  inputs.ctrlassist.url = "github:CtrlAssist/CtrlAssist";

  outputs = { nixpkgs, ctrlassist, ... }: {
    nixosConfigurations.my-machine = nixpkgs.lib.nixosSystem {
      modules = [
        ctrlassist.nixosModules.default
        { services.ctrlassist.enable = true; }
      ];
    };
  };
}
```

Rebuild, then log out and back in. The module installs everything below and starts the daemon and the tray with your session. Three options switch parts off, all on by default:

| Option | What it controls |
|---|---|
| `services.ctrlassist.hiding` | The hiding service. Without it, controllers are grabbed instead; see [How It Works](../reference/how-it-works.md#hiding). |
| `services.ctrlassist.realtime` | Whether users' services may ask for real-time scheduling up to priority 10, which keeps input latency steady on a busy machine. |
| `services.ctrlassist.tray` | The tray, started with the graphical session. |

## Debian, Ubuntu, Fedora and Arch

Each release carries a `.deb`, an `.rpm` and an Arch package, for x86_64 and arm64. Download the one for your distribution from the [releases page](https://github.com/CtrlAssist/CtrlAssist/releases), then:

```sh
{{#include ../generated/release-version.txt}}

# Debian, Ubuntu
sudo apt install ./ctrlassist_${VERSION}-1_amd64.deb

# Fedora
sudo dnf install ./ctrlassist-${VERSION}-1.x86_64.rpm

# Arch
sudo pacman -U ./ctrlassist-${VERSION}-1-x86_64.pkg.tar.zst
```

Log out and back in afterwards: the daemon and the tray start with your session, and the real-time limit takes effect at login.

The packages need glibc 2.34 or newer: Debian 12, Ubuntu 22.04, and any current Fedora or Arch. To check a download before installing it, see [Security & Supply Chain](../reference/security-supply-chain.md).

## What gets installed

- `ctrlassist`, the command, daemon and tray, and `ctrlassist-hider`, the hiding service.
- The daemon and the tray as services of your user, enabled for every user and started at login. To opt out: `systemctl --user mask ctrlassist-daemon ctrlassist-tray`.
- The hiding service as a system service, started the first time it is needed.
- A udev rule giving the person at the machine access to `/dev/uinput` and `/dev/uhid`, which virtual pads are made through, and a setting that loads those and the system's PlayStation controller driver at boot.
- A setting that lets users' services ask for real-time scheduling up to priority 10.
- Shell completions for bash, zsh and fish.

## Check that it works

```sh
ctrlassist status
```

It should say the daemon is alive and which release it runs. Then `ctrlassist pads` lists the controllers it sees. If either disagrees, see [Troubleshooting](../operations/troubleshooting.md).

## GNOME

GNOME has no place for tray icons unless an extension adds one. Install and enable the AppIndicator extension (`gnome-shell-extension-appindicator`) to see the tray. Without it everything else works, notifications included, and you use the command.
