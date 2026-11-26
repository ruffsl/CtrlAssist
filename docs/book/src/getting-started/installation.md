# Installation

CtrlAssist runs on Linux. It installs one command, `ctrlassist`, which is also the daemon and the tray; a settings window, `ctrlassist-gui`; and a small system service that hides controllers from games.

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

<details><summary id="module-without-flakes">Without flakes</summary>

Without flakes, import a fetched copy of the repository (or one pinned with npins or niv); it evaluates to the same outputs the flake has:

```nix
# configuration.nix
let
  ctrlassist = import (builtins.fetchTarball "https://github.com/CtrlAssist/CtrlAssist/archive/main.tar.gz");
in
{
  imports = [ ctrlassist.nixosModules.default ];
  services.ctrlassist.enable = true;
}
```

</details>

<details><summary id="prebuilt-binaries-optional">Prebuilt binaries (optional)</summary>

By default the module builds CtrlAssist against your system's nixpkgs, so it compiles locally on every install and update. Each [release](https://github.com/CtrlAssist/CtrlAssist/releases) is also built by CI and published to the public `ctrlassist.cachix.org` binary cache, for x86_64 and aarch64 Linux. To use those builds instead, pin the repository to a release tag, trust the cache, and point the module at the repository's own package. With flakes:

```nix
# flake.nix
{
  inputs.ctrlassist.url = "github:CtrlAssist/CtrlAssist/vX.Y.Z"; # a release tag
  # No `inputs.ctrlassist.inputs.nixpkgs.follows`: it would change the build
  # and miss the cache.

  outputs = { nixpkgs, ctrlassist, ... }: {
    nixosConfigurations.my-machine = nixpkgs.lib.nixosSystem {
      modules = [
        ctrlassist.nixosModules.default
        ({ pkgs, ... }: {
          nix.settings.substituters = [ "https://ctrlassist.cachix.org" ];
          nix.settings.trusted-public-keys = [
            "ctrlassist.cachix.org-1:bGlpD8WCqOqGDpJH9EEO9YqlCwH1V31N2nNv6IurgNY="
          ];
          services.ctrlassist.enable = true;
          services.ctrlassist.package =
            ctrlassist.packages.${pkgs.stdenv.hostPlatform.system}.default;
        })
      ];
    };
  };
}
```

<details><summary id="prebuilt-without-flakes">Without flakes</summary>

```nix
# configuration.nix
{ pkgs, ... }:
let
  # A release tag.
  ctrlassist = import (builtins.fetchTarball "https://github.com/CtrlAssist/CtrlAssist/archive/vX.Y.Z.tar.gz");
in
{
  imports = [ ctrlassist.nixosModules.default ];
  nix.settings.substituters = [ "https://ctrlassist.cachix.org" ];
  nix.settings.trusted-public-keys = [
    "ctrlassist.cachix.org-1:bGlpD8WCqOqGDpJH9EEO9YqlCwH1V31N2nNv6IurgNY="
  ];
  services.ctrlassist.enable = true;
  services.ctrlassist.package = ctrlassist.packages.${pkgs.stdenv.hostPlatform.system}.default;
}
```

</details>

The `nix.settings` lines take effect once a switch activates them, so pass the cache on the command line for the first switch, or it builds locally (without flakes, drop `--flake`):

```sh
sudo nixos-rebuild switch --flake .#my-machine \
  --option extra-substituters https://ctrlassist.cachix.org \
  --option extra-trusted-public-keys ctrlassist.cachix.org-1:bGlpD8WCqOqGDpJH9EEO9YqlCwH1V31N2nNv6IurgNY=
```

Later switches pick the cache up from `nix.settings` alone.

</details>

Rebuild, then log out and back in. The module installs everything below and starts the daemon and the tray with your session. Three options switch parts off, all on by default:

| Option | What it controls |
|---|---|
| `services.ctrlassist.hiding` | The hiding service. Without it, controllers are grabbed instead; see [How It Works](../reference/how-it-works.md#hiding). |
| `services.ctrlassist.realtime` | Whether users' services may ask for real-time scheduling up to priority 10, which keeps input latency steady on a busy machine. |
| `services.ctrlassist.tray` | The tray, started with the graphical session. |

Two more are for looking into a problem: `services.ctrlassist.logLevel` (`"debug"`, say) sets how much each user's daemon and tray log, and `services.ctrlassist.extraArgs` adds arguments to the daemon's command.

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
- `ctrlassist-gui`, the settings window, with its entry in your desktop's application launcher (**CtrlAssist**) and a session bus file through which the tray's **Settings**, or a second launch, brings the open window to the front.
- The daemon and the tray as services of your user, enabled for every user and started at login. To opt out: `systemctl --user mask ctrlassist-daemon ctrlassist-tray`.
- The hiding service as a system service, started the first time it is needed, with a system user of its own, `ctrlassist-hider`, which it runs as, and a udev rule giving that user what it hides controllers with.
- A udev rule giving the person at the machine access to `/dev/uinput` and `/dev/uhid`, which virtual controllers are made through, and to the raw interface of a [Steam Controller](../guide/steam-controller.md), which CtrlAssist reads it through, and a setting that loads those and the system's PlayStation controller driver at boot.
- A setting that lets users' services ask for real-time scheduling up to priority 10.
- Shell completions for bash, zsh and fish.

## Check that it works

Open **CtrlAssist** from your application menu. At the foot of its sidebar, **Daemon** and **Tray** should each have a green dot, and **Hiding** too where the hiding service is installed, and the **Pads** page should list your controllers. The **About** page says which release each part runs. If something is missing, see [Troubleshooting](../operations/troubleshooting.md).

<details><summary id="check-from-a-terminal">From a terminal</summary>

```sh
ctrlassist status
```

It should say the daemon is alive and which release it runs. Then `ctrlassist pads` lists the controllers it sees.

</details>

## Removing it

On NixOS, set `services.ctrlassist.enable = false` and rebuild. Elsewhere, remove the package with your package manager. A daemon and a tray that are already running may carry on until you log out; to stop yours sooner, which gives back any controllers it hid:

```sh
systemctl --user stop ctrlassist-daemon ctrlassist-tray
```

Your profiles and what CtrlAssist remembers of your controllers stay, in `~/.config/ctrlassist` and `~/.local/state/ctrlassist`; delete those to remove them too.

## GNOME

GNOME has no place for tray icons unless an extension adds one. Install and enable the AppIndicator extension (`gnome-shell-extension-appindicator`) to see the tray. Without it everything else works, notifications included, and you use the command.
