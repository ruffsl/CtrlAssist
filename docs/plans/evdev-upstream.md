# The evdev fixes, out of `vendor/` and on their way upstream

## State

- **Status:** the fixes are seven branches of the fork, reviewed and reworked; this repository builds from the fork's `ctrlassist` branch at a pinned, tagged commit with `vendor/` gone; and the pull requests' text is written. Nothing sent upstream.
- **Next action:** the maintainer opens the seven pull requests on emberian/evdev from the links under [The pull requests](#the-pull-requests).

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
| C | `FFUploadEvent` and `FFEraseEvent` no longer duplicate the virtual device's descriptor: borrowed in the copy, shared through an `Arc` in the fork | `uinput.rs` | Not there | Nothing public changes, as reworked after the review |
| D | `RawDevice::upload_ff_effect` duplicates its descriptor before it uploads, not after | `raw_stream.rs` | Not there | Small |
| E | The resync after `SYN_DROPPED`, in three parts: kept when the read after it finds nothing; the events read with the marker thrown away before the state is fetched, and a frame taken in only once whole; the state fetched into a copy of its own and taken in only when every request answered | `sync_stream.rs` | Not there; their closed #36 and #53 are of this code | The large one, with its own tests |
| F | `Device`'s drop doesn't print when its ungrab fails | `sync_stream.rs` | Not there | Tiny |
| G | Switch and LED events kept in the cached state, which the copy never had | `device_state.rs` | Not there | Small, with a unit test |

## Decided

- **CtrlAssist depends on the fork by a pinned git commit,** `[patch.crates-io] evdev = { git = "https://github.com/CtrlAssist/evdev", rev = "…" }`, and `vendor/` is deleted. Weighed against a submodule and a subtree: a submodule isn't fetched by a flake or by `fetchTarball` unless the person installing asks for it, which the installation page's two NixOS forms don't; a subtree keeps a second copy here to fall out of step with the fork. The pin costs a `rev` here, a hash in the two Nix files and one allowance in `deny.toml`, each changed when the pin moves.
- **The fork's `main` follows upstream's and carries nothing of ours.** Each fix is a branch off it, named for what it does. One more branch, `ctrlassist`, is every fix branch merged, and is what CtrlAssist pins: it is rebuilt, never the place a fix is written.
- **The resync is one pull request of three commits,** and all seven pull requests are opened together, as the maintainer chose; the first plan had the small ones go ahead of the rest.
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
- **How E is tested upstream.** Upstream has both kinds of test (Found): unit tests of the resync that need no device, and device tests of the compensation that its workflow runs as root. E's first two parts can have a device test each; its last, a fetch failing part way, has no way to be provoked on a real device, and is tested here through a seam, which upstream would have to accept in its code.

## Steps

1. **Read upstream as it is.** Run its tests and workflow on its `main` untouched, on this machine and as its CI would; note what they need (uinput, a device), its style, and how its past pull requests were worded and reviewed. Write what was found here. Nothing is changed. Verified by: the notes.
2. **The small fixes, one branch each on a local clone of the fork:** B, D and F, each off upstream's `main`, each with its `CHANGELOG.md` line and a test where one is possible. Verified by: upstream's tests on each branch alone.
3. **C on its branch,** with every use of the two types in the crate and its examples following. Verified by: upstream's tests and examples building.
4. **E as three branches, each on the one before,** in the order they were found: the resync kept on an empty read; the stale events and the whole frame; the state fetched apart. Each with its test. Verified by: upstream's tests, and the tests this repository's copy has for them, carried across.
5. **`ctrlassist` built from all of them, and CtrlAssist pinned to it** in a pull request here: the `[patch]` line, `cargoLock.outputHashes` in `package.nix` and `crate.nix`, the fork allowed in `deny.toml`, `vendor/` and its mentions removed (`docs/build.md`, `packaging/nix/README.md`, `.claude/CLAUDE.md`, `prek.toml`), and what `vendor/README.md` says of each fix moved to where it belongs (`docs/architecture.md` for what the platform was seen to do, the pull requests' text for the rest). Verified by: every gate, the Nix package and the VM checks building, and the tests that exercise each fix through CtrlAssist (the hotplug and resync tests, the fake pads' rumble) still passing. This step needs the branches pushed to the fork, so it waits for the maintainer's word.
6. **The pull requests' text, drafted in this plan,** one section for each, for the maintainer to read and to post or have posted.
7. **Afterwards, as upstream answers:** a fix that is merged leaves `ctrlassist`; when a release has them all, the `[patch]` line, the hash and the allowance go, and this plan is retired into `docs/architecture.md` and `docs/build.md`.

## The pull requests

To be opened on emberian/evdev by the maintainer, all at once, each from its branch of the fork by its link. Written for upstream's readers, in their terms and not this repository's. `#LINT`, `#BUF`, `#FF`, `#RESYNC` and `#SWLED` stand for the numbers of the first, second, fifth, sixth and seventh, to be filled in once they have them, so open the first first. Upstream's changelog is its maintainer's to write, so no branch touches it and each text offers its line.

### 1. `fix/tokio-asyncfd-deprecation`

Open: <https://github.com/emberian/evdev/compare/main...CtrlAssist:evdev:fix/tokio-asyncfd-deprecation?expand=1>

**Title:** Allow the deprecated `AsyncFd::new` so the build passes with current tokio

````markdown
tokio 1.53.3 (and 1.51.6) deprecated `AsyncFd::new` in favor of the new, `unsafe` `AsyncFd::register`. The workflow sets `RUSTFLAGS=-D warnings`, so `cargo clippy --all-features` and `cargo build --all-features` now fail on `main` at the three places an `AsyncFd` is made (`raw_stream.rs`, `sync_stream.rs`, `uinput.rs`).

This allows the lint at those three calls.

The alternative is to switch to `AsyncFd::register`. That would raise the minimum tokio from 1.17 to 1.53.3, and with it the Rust version the `tokio` feature needs (tokio 1.53 declares 1.71), and add three `unsafe` blocks. Its safety condition is that the descriptor stays open and the same while the `AsyncFd` holds it; the crate itself upholds that, but `device_mut()` on each stream hands out `&mut`, so a caller could swap the device. That seemed like your decision to make, so I left the constructor alone. Happy to change this PR if you prefer the switch.
````

### 2. `perf/preallocate-event-buffers`

Open: <https://github.com/emberian/evdev/compare/main...CtrlAssist:evdev:perf/preallocate-event-buffers?expand=1>

**Title:** Allocate event buffers when a device is created

````markdown
`RawDevice` and `VirtualDevice` start with an empty `event_buf` and grow it on the first read. This reserves the space when the device is created instead: one batch (32 events) for `VirtualDevice`, whose fetch drains the buffer, and two for `RawDevice`, where up to a batch can be left over from the previous read when it is read through `Device`.

The effect is that a caller who consumes the events it fetches does not allocate on the read path. We read devices from a real-time thread, where an allocation is a latency risk; I have no benchmark to offer beyond that.

The cost is about 1.5 KB per `RawDevice` that is opened and never read, such as those `enumerate()` opens. If that is a concern, I can make it opt-in instead.

No API change. This touches the same lines of `VirtualDevice::new` as #FF, so whichever lands second needs a trivial rebase.

Suggested changelog line (Changed): `RawDevice` and `VirtualDevice` allocate their event buffers when they are created, not on the first read.

CI note: `cargo clippy --all-features` and `cargo build --all-features` currently fail on `main` under `-D warnings` because tokio deprecated `AsyncFd::new`; #LINT fixes that separately.
````

### 3. `fix/ff-upload-dup-before-upload`

Open: <https://github.com/emberian/evdev/compare/main...CtrlAssist:evdev:fix/ff-upload-dup-before-upload?expand=1>

**Title:** Duplicate the descriptor before uploading a force feedback effect

````markdown
`RawDevice::upload_ff_effect` uploads the effect with `EVIOCSFF` and then duplicates the device's descriptor for the `FFEffect` it returns. If that `try_clone` fails (for example with `EMFILE`), the function returns the error with the effect already on the device and no `FFEffect` to erase it on drop, until the device itself is closed.

This duplicates the descriptor first, so a failure leaves nothing behind.

No test: I couldn't find a way to make `dup` fail on a real device from a test.

Suggested changelog line (Fixed): `RawDevice::upload_ff_effect` no longer leaves an effect on the device when duplicating its file descriptor fails.

CI note: `cargo clippy --all-features` and `cargo build --all-features` currently fail on `main` under `-D warnings` because tokio deprecated `AsyncFd::new`; #LINT fixes that separately.
````

### 4. `fix/drop-without-printing`

Open: <https://github.com/emberian/evdev/compare/main...CtrlAssist:evdev:fix/drop-without-printing?expand=1>

**Title:** Don't print from `Device`'s drop when ungrab fails

````markdown
`Device`'s `Drop` calls `ungrab()` and prints the error with `eprintln!`. Two problems:

- The call fails with `ENODEV` for every device that was unplugged while open, so the message is noise in the most common case. #172 addresses this part.
- `eprintln!` panics if the write to stderr fails, for example with `EPIPE` when stderr is a pipe whose reader has gone. Here that panic happens inside a destructor. (A closed stderr or `/dev/null` is fine; it is a failing write that panics.)

The grab is released when the device is closed regardless, so this ignores the result and prints nothing. It goes one step further than #172, which keeps the print for errors other than `ENODEV`, and it touches the same lines, so the two conflict; I'm glad to rebase onto #172 if you merge that first, or to keep some reporting another way if you'd rather not drop it silently.

Suggested changelog line (Fixed): Dropping a `Device` no longer prints to stderr when ungrabbing fails.

CI note: `cargo clippy --all-features` and `cargo build --all-features` currently fail on `main` under `-D warnings` because tokio deprecated `AsyncFd::new`; #LINT fixes that separately.
````

### 5. `fix/ff-events-share-fd`

Open: <https://github.com/emberian/evdev/compare/main...CtrlAssist:evdev:fix/ff-events-share-fd?expand=1>

**Title:** Share the device's descriptor with force feedback events

````markdown
`VirtualDevice::process_ff_upload` and `process_ff_erase` call `UI_BEGIN_FF_UPLOAD`/`UI_BEGIN_FF_ERASE` and then duplicate the device's descriptor, so that the returned event can call `UI_END_*` when it is dropped. If that `try_clone` fails, the function returns the error after the request has begun and with nothing to end it. The process that asked for the effect then stays blocked in its ioctl until the kernel's 30 second timeout (`uinput_request_submit` in `drivers/input/misc/uinput.c`).

`VirtualDevice` now holds its descriptor in an `Arc`, and the events take a reference to it. Nothing can fail between begin and end, and no descriptor is duplicated per request.

No API change: `FFUploadEvent` and `FFEraseEvent` have no lifetime, and can still be held while the device is used or answered later.

If you'd rather keep the `dup`, a smaller fix is to end the request before returning the `dup` error; I can do it that way instead.

No test: I couldn't find a way to make `dup` fail from a test. This touches the same lines of `VirtualDevice::new` as #BUF.

Suggested changelog line (Fixed): A failed `dup` in `VirtualDevice::process_ff_upload` or `process_ff_erase` no longer leaves the kernel's request unanswered.

CI note: `cargo clippy --all-features` and `cargo build --all-features` currently fail on `main` under `-D warnings` because tokio deprecated `AsyncFd::new`; #LINT fixes that separately.
````

### 6. `fix/resync-after-syn-dropped`

Open: <https://github.com/emberian/evdev/compare/main...CtrlAssist:evdev:fix/resync-after-syn-dropped?expand=1>

**Title:** Fix lost and stale events in the resync after `SYN_DROPPED`

````markdown
Three commits fixing how `Device::fetch_events` and the tokio `EventStream` recover after the kernel drops events. We found these in a daemon that reads gamepads. The first showed up as a test of ours failing every time: a button released inside an overflow stayed held on a uinput gamepad. The others came from reviewing this code against the kernel's `drivers/input/evdev.c` afterwards.

**1. Hand a resync over without reading first.** After fetching the state, `fetch_events_inner` read from the device before returning the resync.
- On a non-blocking device whose queue was emptied by the read that carried the `SYN_DROPPED`, that read fails with `EAGAIN`; the error was returned and the resync, already taken into the cached state, was never produced.
- `EventStream` took the same `EAGAIN` for would-block and lost the resync the same way.
- On a blocking device the read waits for the device's next report with the resync in hand. The existing compensation tests press a key before looking for the resync for this reason.

The resync is now returned as soon as the state is fetched, and the next fetch reads. Tests, each releasing a key inside an overflow and expecting the release with no further report: `test_compensate_without_further_events` (non-blocking), `test_compensate_does_not_wait_for_the_device` (blocking, on a thread so it fails rather than hangs) and `test_compensate_event_stream` (tokio).

**2. Discard stale events, and apply only whole frames.**
- Events read in the same batch as the `SYN_DROPPED`, after the dropped frame, were replayed after the state fetch although they are older than it. The kernel also removes queued key, switch and LED events (not absolute axes) from its buffer when their state is read (`evdev_handle_get_val`), so a replayed press could have had its release removed and stay pressed. The buffer is now cleared before the fetch, as libevdev's `sync_state` drains its queue first. Test: `test_compensate_discards_stale_events`.
- A frame's events were applied to the cached state while the frame was being scanned, before it was known whether part of it had been dropped. The resync is computed against that state, so such a change was never reported; after an overflow the kernel leaves the `SYN_DROPPED` and the newest event, so a key press that happened to be newest was lost. A frame is now applied once its `SYN_REPORT` is reached. Unit test: `test_state_takes_whole_frames_only`.
- A failed state fetch leaves the resync pending instead of clearing it.

**3. Fetch the state into a separate copy.** With (2) making a failed fetch retryable, a fetch that fails part way matters: the state was fetched straight into the cache with up to four ioctls, and if a later one failed (say with `EINTR`), the keys were already written and the retry treated them as already handed to the caller. The state is now fetched into a third `DeviceState` and swapped in only when every ioctl has succeeded. Unit test: `test_resync_takes_nothing_from_a_failed_fetch`.

**Behavior changes to be aware of:**
- A fetch that carries a resync no longer also carries newly read events; they arrive with the next fetch. A non-blocking caller therefore has to fetch until `WouldBlock` to be handed a resync, not once per readiness notification. The `fetch_events` docs now say so.
- Events read after a drop and before the resync are discarded, where they used to be replayed. Key, axis, switch and LED levels are recovered by the state fetch; relative motion and other events with no state are lost, as with libevdev.

The three existing compensation tests pass unchanged. The four new device tests each fail without their commit and pass with it, run as root with `--features device-test,tokio`.

Not addressed here: the cached state ignores switch and LED events between resyncs, which #SWLED fixes separately.

Suggested changelog lines:
- Changed: the fetch after a `SYN_DROPPED` returns only the resync, without reading; events read after the drop are discarded.
- Fixed: the resync after `SYN_DROPPED` is no longer lost on a non-blocking device or an `EventStream` with nothing more to read, no longer waits for the next report on a blocking device, no longer replays stale events, and no longer misses a change that was part of a dropped frame.

CI note: `cargo clippy --all-features` and `cargo build --all-features` currently fail on `main` under `-D warnings` because tokio deprecated `AsyncFd::new`; #LINT fixes that separately.
````

### 7. `fix/track-switch-and-led-state`

Open: <https://github.com/emberian/evdev/compare/main...CtrlAssist:evdev:fix/track-switch-and-led-state?expand=1>

**Title:** Track switch and LED events in the cached device state

````markdown
`DeviceState::process_event` applies key and absolute axis events to the cached state and ignores the rest. The cached state is what a resync after `SYN_DROPPED` is diffed against, and the state fetch does update switches and LEDs, so their cached values only change on a resync and are stale in between.

Example: a resync leaves the cached lid switch at 1; the lid opens and the event is delivered normally (cache still 1); the lid closes inside a dropped frame. The resync compares 1 with 1 and reports nothing, so the caller believes the lid is open.

Switch and LED events now update the state as key events do. Unit test: `test_switch_and_led_events_update_the_state`.

Independent of #RESYNC, though it matters most together with it.

Suggested changelog line (Fixed): the cached `DeviceState` now follows switch and LED events, so a resync reports their changes correctly.

CI note: `cargo clippy --all-features` and `cargo build --all-features` currently fail on `main` under `-D warnings` because tokio deprecated `AsyncFd::new`; #LINT fixes that separately.
````

## Found

- **Upstream as it is, read at `f90916d`** (its `main`, 28 May 2026):
  - **Its checks.** One workflow, on pull requests to `main`: `cargo fmt --check`; `cargo clippy` plain and with every feature, warnings denied by `RUSTFLAGS`; a build and `cargo test`; docs; then the device tests as root, `cargo test --tests --features device-test`. Cross builds for aarch64, arm and i386 run the plain tests under QEMU, and a FreeBSD machine builds and tests.
  - **Its tests.** Five unit tests, two of them of the resync (`sync_stream::tests`), which need no device. Four device tests behind the `device-test` feature, in `tests/`, each making a keyboard through uinput and reading it back with a grab: three of the compensation after a resync (`compensate.rs`), one of a key's state. A fifth, of a virtual device's events, wants the `tokio` feature and isn't run by the workflow's command.
  - **Run here,** in a NixOS virtual machine as root with `uinput` loaded, the binaries built by Nix from a lock file made for the purpose (upstream keeps none): all nine pass. Nothing was made on the host. The expression is kept outside the repository, in the session's scratch directory; step 2 wants it somewhere durable if the fork's branches are to be checked the same way.
  - **Its lint fails on its own `main` today.** With a current tokio, `AsyncFd::new` is deprecated, in three places (`raw_stream.rs`, `sync_stream.rs`, `uinput.rs`), and the workflow denies warnings. So a pull request's checks will be red for a reason that isn't its own until that is fixed, which is one more small pull request to offer, or to wait for. Formatting is clean.
  - **Its maintainers answer.** Fourteen pull requests looked at: small fixes were merged within a day to a few weeks through 2026 (their #177 in a day, #178 in a day, #171 in three, #173 in five weeks), by two people. Four are open, the oldest since December 2025. So it takes pull requests, the small sooner.
- **F is half offered already.** Their open #172, "Squash ENODEV in Device::Drop", from April 2026 with no review yet, stops the print for a device that has gone, which is the case this repository meets. It keeps the print for any other failure, and a print to a closed stream is what panics. So F is a comment on #172, or a pull request after it, not a rival to it.
- **The branches,** each one commit off upstream's `main` at `f90916d` but the resync, which is three, as they stand after the review:

  | Branch | Fix | Tests |
  |---|---|---|
  | `fix/tokio-asyncfd-deprecation` | upstream's failing lint | the workflow's clippy and build with every feature pass with warnings denied, which fail on `main` |
  | `perf/preallocate-event-buffers` | B | builds; nothing to assert without counting allocations |
  | `fix/ff-upload-dup-before-upload` | D | builds; a failing `dup` can't be provoked on a device |
  | `fix/drop-without-printing` | F | builds |
  | `fix/ff-events-share-fd` | C | builds with every feature and the examples; a failing `dup` can't be provoked |
  | `fix/resync-after-syn-dropped` | E | four device tests and two unit tests. The three of its first commit, a key released inside an overflow read non-blocking, blocking and through tokio's stream, each fail on upstream's `main` and pass with it; the stale press's fails without the second commit; the whole-frame unit test fails with that half taken out |
  | `fix/track-switch-and-led-state` | G | a unit test |
  | `ctrlassist` | all of them merged | upstream's formatting and both clippy commands clean; eight unit tests and nine device tests pass in the machine, the tokio ones among them |

  B and C change neighboring lines of `VirtualDevice::new` and conflict when merged, which upstream will meet too; their texts say so.
- **The pin, made and moved** (`7756d65` of `ctrlassist`, tagged `ctrlassist-pin-2026-11-28.2`; first `6b08309`, tagged too): every gate passes, this repository's tests among them, which make pads and read them through the fork's resync and force feedback; the Nix package and the fixture build from it; and the six VM checks pass. The fork's commit is fetched by Nix under a hash in `package.nix` and `crate.nix`.
- **This repository reads a `Device` in one place, and never blocking:** `ctrlassist-device`'s `pad.rs` sets it non-blocking as it is opened. Its other two reads are of a `VirtualDevice`, which has no resync. So a blocking device's resync, below, is upstream's concern and not one this repository meets.
- **The `nix` crate is at two versions with the pin,** where the copy kept it at one: zenoh's Unix socket link pins 0.29, which the copy of evdev 0.13.2 shared, and upstream's `main` has moved to 0.31. The one-version gate failed on it. It is allowed by an entry of its own in `deny.toml`, beside zenoh's other pins, which the gate reports as unnecessary once zenoh catches up.
- **How each fix was come by, from this repository's history,** for what the pull requests may say: E's first part was met, as a test of this repository's own (`events_the_kernel_dropped_cannot_leave_a_button_held`) failing each time once the event buffers started with room (`f64c9e9`); E's second and third parts, C and D were found in the reviews before 0.5.0 and each given a test where one could be (`082294d`, `929c221`, `8c1b42c`); F was reasoned from what `eprintln!` does on a closed stream (`5bd1a23`). None is recorded as seen with a real pad or in a running daemon. That libevdev drains its queue before it syncs is in those commits from memory and was never read.
- **The copy's diff still applies.** Against upstream's `main` every hunk but `sys.rs`'s applies with small offsets; that one is A, which upstream fixed its own way. Upstream's changes to `sync_stream.rs` since the tag are two lines of a call's arguments. So redoing each fix on `main` is a matter of splitting and testing, not of rewriting.

## Deviations

- **After a review by three readers,** each given the code and not what was thought of it, and each finding checked before it was acted on:
  - **No branch touches upstream's changelog.** Its maintainer writes it, in commits of their own, and the lines made four of the branches conflict with one another. Each pull request's text offers its line.
  - **C shares the descriptor where it borrowed it.** A borrow held the device exclusively for as long as a request was being answered: no fetching, no emitting, no answering later, which a caller forwarding the effect elsewhere needs. An `Arc` gives the same (nothing that can fail between a request's begin and its end, and no `dup` for each) with nothing public changed. This repository answers each request before it fetches the next, so either serves it.
  - **E is three commits, and the pin has all of it.** The first and fourth were one change made twice, the fourth deleting what the first added; they are one commit now, which hands a resync over with no read before it, for a non-blocking device, a blocking one and tokio's stream alike, with a test of each. The pin can no longer leave the blocking half out, and needn't: this repository reads until a read would block.
  - **A caller that fetches once for each wakeup is still handed a resync late,** the resync coming with the fetch after the one that met the marker. The fork's docs now say to fetch until a read would block; this repository does (`Pad::read`).
  - **Clearing the buffer before a resync loses what has no state to fetch:** relative motion, and a press and its release that both came after the drop. libevdev does the same. Said in the commit, the docs and the text.
  - **G is new.** Upstream's cached state took in keys and axes and nothing else, so a switch or an LED was only right just after a resync, and the fork's comments claimed more than that. Its own branch and pull request.
  - **What was said and wasn't so:** that `eprintln!` panics on a closed stderr (it is a write that fails, as to a broken pipe, that panics; a closed descriptor doesn't); that nothing can replace a stream's descriptor (`device_mut` lets a caller); that the buffers stop every allocation (only for a caller that takes what it fetches); that only clippy fails on upstream's `main` (its build with every feature does too). Each is corrected where it stood.
  - **The pin is tagged on the fork,** each commit pinned, since `ctrlassist` is rebuilt and a commit no branch reaches may stop being served.
  - **`just evdev-test` builds the tokio tests,** which it had counted as passing with none in them, makes the crate's lock file each time, and takes a checkout whose path has a space.
  - **Left as found:** a Nix hash gone stale after the pin moves is caught by `just vm` or a release and by nothing on a pull request, which `docs/build.md` now says; advisories may not be matched against a crate from git, as they weren't against the copy.

- **Upstream's lint is fixed by allowing the deprecated call,** not by the call tokio now names. `AsyncFd::register` came in tokio 1.53.3, of October 2026, and is unsafe; the crate asks for tokio 1.17 or newer. Moving to it raises that floor past almost every release of tokio, which is upstream's to choose, so the branch allows the lint at the three calls and says why the old one is sound there. The pull request's text should offer the other way.
- **(Before the review; see above for where it ended.) E has a fourth part, for a blocking device, which the copy never had.** `fix/resync-returned-without-reading`, on the third: a resync is handed over as soon as the state is fetched, with no read before it, and the next fetch reads. Its device test releases a key inside an overflow on a blocking device and gives the resync two seconds, on a thread of its own so that it fails where it would hang; it fails without the change and passes with it, and upstream's own three tests of the compensation pass unchanged. A fetch that carries a resync then carries no newly read events, which is the one thing a caller could notice. It is not in `ctrlassist`, by the maintainer's choice: this repository never reads a blocking device, and the pin stays on what it has run. Why it was wanted:
- **E's first part is carried as it is here, and leaves a blocking device as it was.** On a device that isn't set non-blocking, the read after a resync waits for the device's next report with the resync in hand, where a non-blocking one failed with `EAGAIN`. Upstream's own tests are written around that wait: they press a key again before they look for the resync. Not reading at all while a resync is owed would cover both, and is a larger change to offer in the pull request's discussion than to make unasked.
