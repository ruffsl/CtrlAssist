# Nix packaging

CtrlAssist's package derivation, its NixOS module, the virtual machine checks, and the tools the dev shell pins. The definitions are the flake's outputs. After TisVlay's `packaging/nix/README.md`, trimmed: CtrlAssist has no binary cache and no `default.nix` for plain Nix yet.

## Contents

| File | Purpose |
|---|---|
| [`package.nix`](package.nix) | `rustPlatform.buildRustPackage` for the shipping binaries: `ctrlassist`, `ctrlassist-hider` and `ctrlassist-gui`, with the units, the udev rule, the window's desktop entry and session bus file, the license bundle and the completions. |
| [`module.nix`](module.nix) | The NixOS module: `services.ctrlassist.enable`, and the `hiding`, `realtime` and `tray` switches. |
| [`crate.nix`](crate.nix) | One workspace crate's binaries as a package, for what the checks need before the full package, such as the test crate's `ctrlassist-fixture`. |
| [`slint-runtime.nix`](slint-runtime.nix) | The libraries the window needs at run time beyond what the linker records, shared by the package's rpath and the dev shell. |
| [`cargo-about.nix`](cargo-about.nix) | cargo-about pinned to the release that keeps the Slint crates' license text, for the dev shell. |
| [`slint-lsp.nix`](slint-lsp.nix), [`slint-viewer.nix`](slint-viewer.nix) | The dev shell's Slint language server and viewer, at the workspace's Slint version. |
| `test-*.nix` | The virtual machine checks, one flake check each; what each asserts is in [docs/testing.md](../../docs/testing.md#virtual-machine-checks). |

The units, the udev rule and the session bus file it installs are the distro packages' own, in [`../systemd/`](../systemd/) and [`../udev/`](../udev/), with `/usr/bin` pointed at the package; `--replace-fail` fails the build if one drifts from that.

## Using the module

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

The user-facing account, with what each switch does, is the book's [Installation](../../docs/book/src/getting-started/installation.md#nixos) page. Every option has a description in [`module.nix`](module.nix), which is the authoritative reference.

The module's default `package` is `pkgs.ctrlassist` when an overlay provides it, else this directory's `package.nix` built against the host's nixpkgs, with the `release` profile. The flake's own `packages.default` passes the `dist` profile, as the deb, rpm and Arch packages use.

## How the package builds

- **`cargoLock.lockFile`,** not a `cargoHash`, so a dependency bump needs no re-hash here.
- **Which crates are built** comes from `workspace.default-members` in the root `Cargo.toml`, the shipping binaries, so the list lives in one place.
- **No tests in the build** (`doCheck = false`): the workspace's tests run in the dev shell and in CI, and the virtual machine checks run the package itself.
- **The source is a fileset** of what the build reads, so a change to the docs doesn't rebuild it.
- **The license bundle** is written in `preBuild` by cargo-about from the vendored sources, with no network.
- **The window's run-time libraries** go on its rpath by hand in `postFixup` (`slint-runtime.nix`), and the usual pass that would take off what it finds no use of is skipped, since patchelf can't see a library opened by name.

## Virtual machine checks

```bash
just vm daemon       # or platform, device, hider, gui
nix flake check      # all of them, with the rest of the flake's checks
```

They need KVM and take a minute or two each. See [docs/testing.md](../../docs/testing.md#virtual-machine-checks).
