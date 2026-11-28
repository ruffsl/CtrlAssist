# More Nix in CI

## Goal

CI runs on GitHub's Ubuntu runners with tools installed and pinned by hand, and the six virtual machine checks, which are where the platform is really exercised, run only on a developer's machine. Two things follow from that:

- **Code that needs a newer kernel is never run by CI.** A fake Steam Controller needs Linux 7.3's `hid-steam`, the runner's kernel is older, and every test of it skips there. On the Steam Controller's pull request that left 417 new lines uncounted and the patch coverage check failing at 88.8% against a target of 92.1%. It may be years before 7.3 is a runner's kernel.
- **CI and a developer's shell agree only by care.** Eight tool versions in `ci.yml` and `build-packages.yml` are kept equal to the flake's by a comment that says to.

This plan is research first: what using Nix for more of CI would cost and give, tried on a branch from `main` so that the Steam Controller's work doesn't wait on it. Nothing below is decided.

## State

Research written, 2026-11-28. No step started. Next: the maintainer picks which trials to run.

## What is there now

| Job | Runs on | Nix |
|-----|---------|-----|
| `checks` | Ubuntu, prek with the hygiene hooks and the workflow linters | none |
| `ci` | Ubuntu: apt libraries, rustup, eight tools by `taiki-e/install-action`, prek, then `just test-ci` (clippy, nextest and doc tests under coverage, the floor), Codecov | none |
| `msrv` | Ubuntu, rustup with the floor from `Cargo.toml` | none |
| `docs` | Ubuntu, mdBook | none |
| `build-packages` | Ubuntu, the deb, rpm and Arch packages | none |
| `release` → `nix-cache` | Ubuntu, `cachix/install-nix-action`, builds `.#default` and pushes it to `ctrlassist.cachix.org` | yes, on a `v*` tag alone |

The virtual machine checks (`platform`, `device`, `hider`, `hider-old-kernel`, `daemon`, `gui`) are flake checks run by `just vm <name>`. Two pin Linux 7.3 and one 6.12. None runs a cargo test: each drives the built package or the fixture.

TisVlay, the template, is the same: no Nix in CI but the release's cache push, its one virtual machine check run by hand. So anything here is a departure from it, or something to carry back to it.

## Found

**About the runners** (to be confirmed by the first trial, not taken from these pages):

- A standard runner for a public repository has 4 CPUs, 16 GB of memory and 14 GB of disk, free and without a limit of minutes ([GitHub's runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners)).
- KVM works on them. It needs a udev rule to give the runner's user `/dev/kvm`, which Determinate's installer action writes by default and which is three lines with any other ([Determinate's note](https://determinate.systems/blog/kvm-on-github-actions), [a write-up of NixOS VMs in Actions](https://jnsgr.uk/2024/02/nixos-vms-in-github-actions)). So a NixOS test runs there at something like a desktop's pace per CPU, on four of them.
- The Actions cache holds 10 GB for a repository by default, the oldest entries evicted ([limits](https://docs.github.com/en/actions/reference/limits)).

**About caches:**

- `ctrlassist.cachix.org` exists, and its push token is in an environment only a `v*` tag may use. A pull request can read it and can't write it, and a fork's never could. Cachix's free allowance for open source was read as 5 GB in one place and 10 GB in another; unconfirmed.
- The Nix store can be kept in the Actions cache with no token at all: `nix-community/cache-nix-action`, or Determinate's Magic Nix Cache, which was withdrawn and then brought back on GitHub's newer cache API. Both share the 10 GB with `Swatinem/rust-cache`.
- Garnix, a hosted CI for flakes that would have run the checks with no workflow at all, is reported to be shutting down ([the thread](https://discourse.nixos.org/t/garnix-is-shutting-down-not-oc/77895/6)). Not an option to build on.

**Measured here:**

- The dev shell's closure is 6.2 GiB, against a runner's 14 GB of disk. Most of it is in `cache.nixos.org`; `slint-lsp`, `slint-viewer`, the pinned `cargo-about` and the tombi override are built from source and are in no cache a pull request can read unless one is given them.
- The package's closure is 257 MiB.
- The package and the fixture are `rustPlatform.buildRustPackage`: one derivation each, so any change to the source builds the whole workspace and every dependency again, with nothing kept between runs. On a 32-thread desktop that is minutes; on four CPUs it hasn't been timed, and it is the cost every virtual machine check pays first.

## The four questions

### The virtual machine checks in CI

What it gives: the platform's checks gate a pull request, not the maintainer's memory to run them. Hiding, the daemon as installed, the window, and the Steam Controller's driver on 7.3 are all checked nowhere else.

What it costs: a Nix install, the package and fixture built from nothing, six machines booted. The build is the unknown.

- **As they are,** one job per check or one job for all, with the store kept in the Actions cache. Simplest; the build is paid whenever the source changes, which in a pull request is every push.
- **With the dependencies in a derivation of their own** (crane, or `buildRustPackage` with a vendored dependency build split out), so that a push builds the workspace's own crates alone. A change to `package.nix` and `crate.nix`, which the NixOS module's users also build through; more to keep.
- **Only when asked or when it matters:** on a label, on `main`, or when `packaging/`, `crates/ctrlassist-hider/` or `crates/ctrlassist-device/` change, which the `changes` job could say.

### Cargo tests on Linux 7.3

The tests that skip on the runner are ordinary nextest tests that need a kernel, uhid and udev. Three ways to give them one:

- **A NixOS test that runs them.** The tests are built in a derivation (`cargo nextest archive`), the machine boots 7.3 and runs the archive. It is the shape `just evdev-test` already has for the evdev fork's tests. Everything the Steam Controller's tests need is the machine's to give: the udev rule, no Steam, no browser.
- **The same, counted.** The archive is built under `cargo llvm-cov show-env`, the machine writes the profile data out, a second lcov is made from it and uploaded to Codecov beside the runner's, which adds reports for one commit together. The paths in it are the Nix build's and would have to be mapped back to the repository's. More parts, and the only option here that closes the coverage gap for kernel-gated code.
- **A newer kernel under the runner's own tests, without Nix.** `virtme-ng` boots a chosen kernel in QEMU with the host's file system as the guest's, so `just test-ci` would run as it does now, coverage and all, inside it. Whether it can fetch a built 7.3 with `hid-steam`, `hid-playstation`, uhid and uinput as modules, and whether udev and the nodes' access behave in such a guest, is unread and untried. If it works it is the smallest change and makes the runner's kernel a line in the workflow.

### The dev shell for every job

What it gives: one list of tools. The eight pins and their comment go, `rustup` goes, the apt libraries go, and "works in the shell" means "works in CI".

What it costs: 6.2 GiB fetched or restored on each job that uses it, some of it built from source unless cached, on a 14 GB disk that also holds `target/`. A shell for CI alone, without the window's tools, the docs' and the hardware tools, would be a fraction of that, and is a second list to keep, though one in the same file as the first.

What it doesn't change: the tests still run on the runner's kernel.

### Cost, and what else there is

- **Minutes** are free for a public repository; the cost is how long a pull request waits. `ci` takes about ten minutes now.
- **Disk** is the hard limit: 14 GB for the store, the cargo target and a virtual machine's image at once.
- **Cache** is 10 GB shared between cargo's and Nix's, or Cachix with a token pull requests may use, which means a second cache or a token of narrower trust than the release's.
- **Staying as it is** costs nothing new and leaves both problems in Goal.
- **Self-hosted runners** would give any kernel and a warm store, and are a machine to keep and to trust with pull requests from strangers. Not looked at further.

## Decisions

None yet.

## Open

- Which of the trials below to run, and in what order.
- Whether the virtual machine checks should gate every pull request or only some.
- Whether a departure from TisVlay here is to be carried back to it.
- What Codecov's patch check does meanwhile for code CI can't run: fail as now, report without failing, or leave such files out of the figure. Put off until the trials say whether the gap can be closed.

## Steps

Each trial is a draft pull request from `main` that changes workflows alone, and ends with what was measured written into Found. The maintainer reads each finding before the next starts.

1. **Time the checks on a runner.** One workflow, run by hand: install Nix, give the user KVM, build `.#default` and `.#fixture`, run the six checks, each step timed, with and without the store restored from the Actions cache. Says whether the build needs splitting and whether the disk holds.
2. **Try `virtme-ng` under `just test-ci`.** One job on a branch that has the Steam Controller's tests: does a fetched 7.3 boot, do the fake pads appear with their nodes, do the skipped tests run, does the lcov come out whole. Says whether the coverage gap closes without Nix.
3. **Try the tests in a NixOS machine,** if 2 fails: the archive built by Nix and run on 7.3, first uncounted, then with coverage.
4. **Time the dev shell on a runner:** `nix develop -c just test-ci` beside the job as it is, cold and restored, and the same with a shell cut down for CI.
5. **Decide,** and write the build steps that follow from it here.

## Deviations

None yet.
