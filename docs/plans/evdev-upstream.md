# The evdev fixes, out of `vendor/` and on their way upstream

## State

- **Status:** every fix is a branch in a local clone of the fork (steps 2 to 4), tested; nothing pushed to the fork, nothing sent upstream.
- **Next action:** the maintainer's read of the branches, in `../evdev` beside this repository, and their word to push them to the fork.

## Goal

CtrlAssist carries a copy of `evdev` 0.13.2 under `vendor/` with six fixes that upstream hasn't, each written up in `vendor/README.md` and none reported. A copy in this repository is the wrong home for them: nobody upstream can see them, the copy has lost the crate's own tests and examples, and it falls behind upstream's `main` with every commit there.

The fixes move to branches of a fork, [CtrlAssist/evdev](https://github.com/CtrlAssist/evdev), each rebased onto upstream's `main` with the crate's tests passing, so that each can be offered to [emberian/evdev](https://github.com/emberian/evdev) as a pull request. CtrlAssist builds from the fork at a pinned commit meanwhile, and from a release once one has them. `vendor/` goes.

## What is there now

- **The copy:** `vendor/evdev`, 0.13.2, changed in `src/sys.rs`, `src/raw_stream.rs`, `src/uinput.rs` and `src/sync_stream.rs`, some 226 lines of diff against the published source. Its examples, tests and development dependencies were left out, and two of its unit tests were rewritten to do without `itertools`.
- **Upstream:** no release since 0.13.2. Its `main` is 11 commits past the tag, the last in May 2026, and touches the same four files lightly (26 lines added, 10 removed among them). It moved `nix` from 0.29 to 0.31, declares Rust 1.64 as its least, has a `CHANGELOG.md`, three integration tests under `tests/` and one workflow.
- **The fork:** `CtrlAssist/evdev`, a fork of `emberian/evdev`, on `main`, with nothing of ours on it.
- **How this repository builds it:** `[patch.crates-io] evdev = { path = "vendor/evdev" }`. The Nix package and the VM checks' fixture take `vendor/` into their source and read dependencies from `Cargo.lock` (`cargoLock.lockFile`). cargo-deny refuses every git source (`unknown-git = "deny"`). prek leaves `vendor/` out of two hooks.

## The fixes

| | Fix | Files | Upstream | As a pull request |
|---|---|---|---|---|
| A | `UI_SET_PHYS` declared with the size of a character, so `with_phys` always failed | `sys.rs` | Fixed on `main` (their #163), unreleased | None: it falls away on the rebase |
| B | `RawDevice` and `VirtualDevice` start with room for their event buffers | `raw_stream.rs`, `uinput.rs` | Not there | Small, nothing public changes |
| C | `FFUploadEvent` and `FFEraseEvent` borrow the virtual device's descriptor, where they duplicated it | `uinput.rs` | Not there | Breaking: both types gain a lifetime |
| D | `RawDevice::upload_ff_effect` duplicates its descriptor before it uploads, not after | `raw_stream.rs` | Not there | Small |
| E | The resync after `SYN_DROPPED`, in three parts: kept when the read after it finds nothing; the events read with the marker thrown away before the state is fetched, and a frame taken in only once whole; the state fetched into a copy of its own and taken in only when every request answered | `sync_stream.rs` | Not there; their closed #36 and #53 are of this code | The large one, with its own tests |
| F | `Device`'s drop doesn't print when its ungrab fails | `sync_stream.rs` | Not there | Tiny |

## Decided

- **CtrlAssist depends on the fork by a pinned git commit,** `[patch.crates-io] evdev = { git = "https://github.com/CtrlAssist/evdev", rev = "…" }`, and `vendor/` is deleted. Weighed against a submodule and a subtree: a submodule isn't fetched by a flake or by `fetchTarball` unless the person installing asks for it, which the installation page's two NixOS forms don't; a subtree keeps a second copy here to fall out of step with the fork. The pin costs a `rev` here, a hash in the two Nix files and one allowance in `deny.toml`, each changed when the pin moves.
- **The fork's `main` follows upstream's and carries nothing of ours.** Each fix is a branch off it, named for what it does. One more branch, `ctrlassist`, is every fix branch merged, and is what CtrlAssist pins: it is rebuilt, never the place a fix is written.
- **One pull request for each fix, the resync's three parts as a stack.** The small ones (B, D, F) can be taken while the larger are talked over. C is offered apart from the rest because it breaks the public interface and so waits for upstream's next minor release.
- **Upstream's tests are run in a virtual machine, never on the maintainer's desktop,** as this repository's own are kept from making pads while Steam is open: they make keyboards through uinput and grab them.
- **Each fix is redone on upstream's `main`,** not carried across as the diff it is here: written against the current code, with the crate's own tests and workflow passing, upstream's style and its least Rust kept, a line in its `CHANGELOG.md`, and a test of the fix where one can be written without a device.
- **Nothing reaches upstream without the maintainer's word,** and nothing is pushed to the fork before this plan is read. Pull requests upstream are opened by the maintainer, or by Claude on their say for each one; their text is drafted here first.
- **Upstream's failing lint is offered a fix first,** so that the pull requests after it are checked for what they change.
- **F is offered as a pull request of its own,** naming their #172, which stops the print for a device that has gone and keeps it otherwise.
- **The machine that runs the crate's tests is kept here,** `packaging/nix/evdev-tests.nix` behind `just evdev-test`, and leaves with this plan.
- **An issue goes with a pull request only where the bug wants explaining:** E, whose three parts are each a sequence of kernel and library steps. The others say what is wrong in their own description.

## Open

- **If upstream stops answering.** It takes pull requests today (Found), though it has made no release since 0.13.2. If ours sit unanswered, or are merged and never released, the pin on the fork is where CtrlAssist stays, and the question becomes whether to publish the fork under another name. To be decided when it happens, by the maintainer's choice.
- **What C should look like to upstream.** A lifetime on two public types is the smallest change that never duplicates a descriptor. Upstream may prefer a new pair of types beside the old, or the old kept with the failure of `dup` handled; the pull request should say what CtrlAssist needs (no descriptor duplicated on the path a game's request takes) and offer the lifetime as one way.
- **How E is tested upstream.** Upstream has both kinds of test (Found): unit tests of the resync that need no device, and device tests of the compensation that its workflow runs as root. E's first two parts can have a device test each; its last, a fetch failing part way, has no way to be provoked on a real device, and is tested here through a seam, which upstream would have to accept in its code.
- **The `nix` crate.** `Cargo.lock` has both 0.29 and 0.31 today. Upstream's `main` uses 0.31, so the move may leave one; to be read from the lock file in step 5, with cargo-deny's one-version gate.

## Steps

1. **Read upstream as it is.** Run its tests and workflow on its `main` untouched, on this machine and as its CI would; note what they need (uinput, a device), its style, and how its past pull requests were worded and reviewed. Write what was found here. Nothing is changed. Verified by: the notes.
2. **The small fixes, one branch each on a local clone of the fork:** B, D and F, each off upstream's `main`, each with its `CHANGELOG.md` line and a test where one is possible. Verified by: upstream's tests on each branch alone.
3. **C on its branch,** with every use of the two types in the crate and its examples following. Verified by: upstream's tests and examples building.
4. **E as three branches, each on the one before,** in the order they were found: the resync kept on an empty read; the stale events and the whole frame; the state fetched apart. Each with its test. Verified by: upstream's tests, and the tests this repository's copy has for them, carried across.
5. **`ctrlassist` built from all of them, and CtrlAssist pinned to it** in a pull request here: the `[patch]` line, `cargoLock.outputHashes` in `package.nix` and `crate.nix`, the fork allowed in `deny.toml`, `vendor/` and its mentions removed (`docs/build.md`, `packaging/nix/README.md`, `.claude/CLAUDE.md`, `prek.toml`), and what `vendor/README.md` says of each fix moved to where it belongs (`docs/architecture.md` for what the platform was seen to do, the pull requests' text for the rest). Verified by: every gate, the Nix package and the VM checks building, and the tests that exercise each fix through CtrlAssist (the hotplug and resync tests, the fake pads' rumble) still passing. This step needs the branches pushed to the fork, so it waits for the maintainer's word.
6. **The pull requests' text, drafted in this plan,** one section for each, for the maintainer to read and to post or have posted.
7. **Afterwards, as upstream answers:** a fix that is merged leaves `ctrlassist`; when a release has them all, the `[patch]` line, the hash and the allowance go, and this plan is retired into `docs/architecture.md` and `docs/build.md`.

## Found

- **Upstream as it is, read at `f90916d`** (its `main`, 28 May 2026):
  - **Its checks.** One workflow, on pull requests to `main`: `cargo fmt --check`; `cargo clippy` plain and with every feature, warnings denied by `RUSTFLAGS`; a build and `cargo test`; docs; then the device tests as root, `cargo test --tests --features device-test`. Cross builds for aarch64, arm and i386 run the plain tests under QEMU, and a FreeBSD machine builds and tests.
  - **Its tests.** Five unit tests, two of them of the resync (`sync_stream::tests`), which need no device. Four device tests behind the `device-test` feature, in `tests/`, each making a keyboard through uinput and reading it back with a grab: three of the compensation after a resync (`compensate.rs`), one of a key's state. A fifth, of a virtual device's events, wants the `tokio` feature and isn't run by the workflow's command.
  - **Run here,** in a NixOS virtual machine as root with `uinput` loaded, the binaries built by Nix from a lock file made for the purpose (upstream keeps none): all nine pass. Nothing was made on the host. The expression is kept outside the repository, in the session's scratch directory; step 2 wants it somewhere durable if the fork's branches are to be checked the same way.
  - **Its lint fails on its own `main` today.** With a current tokio, `AsyncFd::new` is deprecated, in three places (`raw_stream.rs`, `sync_stream.rs`, `uinput.rs`), and the workflow denies warnings. So a pull request's checks will be red for a reason that isn't its own until that is fixed, which is one more small pull request to offer, or to wait for. Formatting is clean.
  - **Its maintainers answer.** Fourteen pull requests looked at: small fixes were merged within a day to a few weeks through 2026 (their #177 in a day, #178 in a day, #171 in three, #173 in five weeks), by two people. Four are open, the oldest since December 2025. So it takes pull requests, the small sooner.
- **F is half offered already.** Their open #172, "Squash ENODEV in Device::Drop", from April 2026 with no review yet, stops the print for a device that has gone, which is the case this repository meets. It keeps the print for any other failure, and a print to a closed stream is what panics. So F is a comment on #172, or a pull request after it, not a rival to it.
- **The branches,** each off upstream's `main` at `f90916d`, in the clone at `../evdev`:

  | Branch | Fix | Tests |
  |---|---|---|
  | `fix/tokio-asyncfd-deprecation` | upstream's failing lint | the workflow's two clippy commands pass with warnings denied, which fail on `main` |
  | `perf/preallocate-event-buffers` | B | builds; nothing to assert without counting allocations |
  | `fix/ff-upload-dup-before-upload` | D | builds; a failing `dup` can't be provoked on a device |
  | `fix/drop-without-printing` | F | builds |
  | `feat/ff-events-borrow-fd` | C | builds with every feature and the examples, `virtual_ff` among them unchanged |
  | `fix/resync-kept-on-empty-read` | E, first part | a device test, `test_compensate_without_further_events`: a key released inside an overflow on a non-blocking device. Fails on upstream's `main` (the release never comes), passes with the fix |
  | `fix/resync-discards-stale-events`, on the one above | E, second part | a device test, `test_compensate_discards_stale_events` (fails without the fix: the key reads as pressed), and a unit test, `test_state_takes_whole_frames_only` (fails with that half of the fix taken out) |
  | `fix/resync-fetches-state-apart`, on the one above | E, third part | the unit test this repository's copy has, carried across: a fetch that fails after the keys |
  | `ctrlassist` | all of them merged | upstream's formatting and both clippy commands clean; seven unit tests and six device tests pass in the machine; this repository's workspace compiles against it with every target (`cargo check`, the patch given on the command line and `Cargo.lock` put back) |

  Each has a line in upstream's `CHANGELOG.md` under its next release, which makes every merge into `ctrlassist` conflict there; they are resolved by keeping both sides, and one commit on `ctrlassist` puts the lines under their headings.
- **Not tried:** this repository's own tests against `ctrlassist`. They make pads, and the pin they would run through is step 5.
- **The `nix` crate stays at two versions** with `ctrlassist` in place of the copy: something else in the workspace asks for 0.29. Not looked into further.
- **The copy's diff still applies.** Against upstream's `main` every hunk but `sys.rs`'s applies with small offsets; that one is A, which upstream fixed its own way. Upstream's changes to `sync_stream.rs` since the tag are two lines of a call's arguments. So redoing each fix on `main` is a matter of splitting and testing, not of rewriting.

## Deviations

- **Upstream's lint is fixed by allowing the deprecated call,** not by the call tokio now names. `AsyncFd::register` came in tokio 1.53.3, of October 2026, and is unsafe; the crate asks for tokio 1.17 or newer. Moving to it raises that floor past almost every release of tokio, which is upstream's to choose, so the branch allows the lint at the three calls and says why the old one is sound there. The pull request's text should offer the other way.
- **E's first part is carried as it is here, and leaves a blocking device as it was.** On a device that isn't set non-blocking, the read after a resync waits for the device's next report with the resync in hand, where a non-blocking one failed with `EAGAIN`. Upstream's own tests are written around that wait: they press a key again before they look for the resync. Not reading at all while a resync is owed would cover both, and is a larger change to offer in the pull request's discussion than to make unasked.
