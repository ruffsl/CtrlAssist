# Nix packaging

CtrlAssist's package derivation, its NixOS module, the virtual machine checks, and the tools the dev shell pins. The definitions are the flake's outputs. After TisVlay's `packaging/nix/README.md`.

## Contents

| File | Purpose |
|---|---|
| [`package.nix`](package.nix) | `rustPlatform.buildRustPackage` for the shipping binaries: `ctrlassist`, `ctrlassist-hider` and `ctrlassist-gui`, with the units, the udev rule, the window's desktop entry and session bus file, the license bundle and the completions. |
| [`module.nix`](module.nix) | The NixOS module: `services.ctrlassist.enable`, and the `hiding`, `realtime` and `tray` switches. |
| [`../../default.nix`](../../default.nix) | flake-compat shim: the flake's outputs for plain Nix (`nix-build`, `import`). |
| [`crate.nix`](crate.nix) | One workspace crate's binaries as a package, for what the checks need before the full package, such as the test crate's `ctrlassist-fixture`. |
| [`slint-runtime.nix`](slint-runtime.nix) | The libraries the window needs at run time beyond what the linker records, shared by the package's rpath and the dev shell. |
| [`cargo-about.nix`](cargo-about.nix) | cargo-about pinned to the release that keeps the Slint crates' license text, for the dev shell and the package's own license bundle. |
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

The user-facing account, with what each switch does, the plain-Nix `import` and the cached release builds, is the book's [Installation](../../docs/book/src/getting-started/installation.md#nixos) page. Every option has a description in [`module.nix`](module.nix), which is the authoritative reference.

The module's default `package` is `pkgs.ctrlassist` when an overlay provides it, else this directory's `package.nix` built against the host's nixpkgs, with the `release` profile. The flake's own `packages.default` passes the `dist` profile, as the deb, rpm and Arch packages use.

## Binary cache

Release builds of the flake's package are in the public [ctrlassist.cachix.org](https://ctrlassist.cachix.org) cache (public key `ctrlassist.cachix.org-1:bGlpD8WCqOqGDpJH9EEO9YqlCwH1V31N2nNv6IurgNY=`), for x86_64-linux and aarch64-linux.

- **What's pushed:** `packages.${system}.default`, built from the flake's own `flake.lock`, and only its runtime closure. Build-only paths such as the vendored crate sources stay out, and so does anything cache.nixos.org already serves. Each build is pinned as `<tag>-<system>` (`v0.5.0-x86_64-linux`, say), keeping one revision, so eviction can't drop a release, and a re-pushed tag moves the pin to its new build.
- **When:** the `nix-cache` job in [`release.yml`](../../.github/workflows/release.yml) runs on every `v*` tag push, beside the distro package build. Nothing on `main` or in pull requests is pushed.
- **Access:** the push token (`CACHIX_AUTH_TOKEN`) lives in the `cachix` environment, which only admits `v*` tags and needs the maintainer's approval. The job builds from cache.nixos.org alone and never substitutes from this cache, so it only writes to it.
- **Budget:** Cachix's free open-source plan holds 5 GB and evicts the least recently used paths past that. Paths already on cache.nixos.org aren't stored again.
- **Opt-in, by design:** a cache only hits when the user's derivation hashes to what CI built, which means the flake's package from this repository's `flake.lock`. The module's default package builds against the host's nixpkgs and never matches, and it stays the default: it shares the system's libraries, the host's security updates reach it, and the window's renderer stays paired with the system's Mesa. Users who want prebuilt binaries set `services.ctrlassist.package` to the flake's package and accept a second set of runtime libraries (the book's Installation page has the steps).

## How the package builds

- **`cargoLock.lockFile`,** not a `cargoHash`, so a dependency bump needs no re-hash here.
- **Which crates are built** comes from `workspace.default-members` in the root `Cargo.toml`, the shipping binaries, so the list lives in one place.
- **No tests in the build** (`doCheck = false`): the workspace's tests run in the dev shell and in CI, and the virtual machine checks run the package itself.
- **The source is a fileset** of what the build reads, so a change to the docs doesn't rebuild it.
- **The license bundle** is written in `preBuild` by the pinned cargo-about (`cargo-about.nix`) from the vendored sources, with no network.
- **The window's run-time libraries** go on its rpath by hand in `postFixup` (`slint-runtime.nix`), and the usual pass that would take off what it finds no use of is skipped, since patchelf can't see a library opened by name.

## Virtual machine checks

```bash
just vm daemon       # or platform, device, hider, gui
nix flake check      # all of them, with the rest of the flake's checks
```

They need KVM and take a minute or two each. See [docs/testing.md](../../docs/testing.md#virtual-machine-checks).
