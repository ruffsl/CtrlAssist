# Finding: ecosystem survey

Surveyed 2026-11-21. Release data is from the crates.io API on that date; changelogs were read from the published crate archives; InputPlumber's manifest from its `main` branch. Anything not read from one of those is marked unverified.

The survey chooses nothing. It gives each spike its candidates and flags what changed since the legacy dependency set was chosen.

## Summary

- **Nothing in the legacy set is abandoned, and nothing was superseded outright.** The legacy lockfile was already on the newest `evdev`, `udev` and nearly the newest `gilrs`. The stale ones are the desktop crates (`ksni`, `notify-rust`, `ashpd`, `zbus`), which TisVlay's pins already cover.
- **gilrs has barely moved** and offers no new help for this project: its last two releases are mapping updates plus the two additions made for CtrlAssist 0.4.0.
- **There is no Rust crate that emulates a DualSense.** The building blocks are a thin uhid binding and several host-side DualSense protocol crates. InputPlumber has a full implementation, but it is GPL-3.0-or-later, so it can be studied and talked to, not copied.
- **One new candidate matters:** `evdevil`, a second evdev and uinput binding with force feedback, hotplug and async built in.
- **InputPlumber is closer prior art than assumed:** a root system daemon in Rust that routes pads to virtual pads, with uhid targets, udev-based hiding and a ratatui TUI.

## Legacy dependencies

| Crate | Legacy lock | Current | Last release | Notes |
|---|---|---|---|---|
| `gilrs` | 0.11.1 | 0.11.2 | 2026-05-30 | 0.11.2: MSRV 1.84, mapping update. 0.11.1 (2026-01) added `LinuxGamepadExt::devpath` and `GilrsBuilder::with_force_feedback`, both used by 0.4.0. Before that, 0.11.0 is from 2024-09. Bundles SDL_GameControllerDB in the crate. Apache-2.0/MIT. |
| `gilrs-core` | 0.6.7 | 0.6.8 | 2026-05-30 | Linux change: nix 0.31. |
| `evdev` | 0.13.2 | 0.13.2 | 2025-09-15 | No release in 14 months, but the repository is active (last commit 2026-05-28, adding the Steam Deck back-paddle button codes). InputPlumber depends on a git revision rather than the release, which suggests the release lacks things it needs; which things is unverified. Apache-2.0 OR MIT. |
| `udev` | 0.9.3 | 0.9.3 | 2025-01-23 | libudev bindings (links the C library). MIT. `tokio-udev` 0.10.1 (2026-11-17) wraps it for async. |
| `ksni` | 0.3.3 | 0.3.7 | 2026-10-20 | TisVlay pins `0.3`. |
| `notify-rust` | 4.11.7 | 4.19.0 | 2026-11-08 | TisVlay pins `4` with `z-with-tokio`. |
| `ashpd` | 0.12.1 | 0.14.1 | 2026-11-19 | Legacy used it only for `is_sandboxed`. |
| `fs2` | 0.4.3 | 0.4.3 | 2018-01-06 | Unmaintained. `fs4` 1.1.0 (2026-04-28) is its maintained successor. Only needed if Steam's config is still edited. |
| `dirs` | 6 | 7.0.0 | 2026-09-05 | TisVlay uses `directories` 6 instead. |
| `signal-hook` | 0.4 | 0.4.6 | 2026-10-31 | TisVlay uses tokio's signal handling. |
| `parking_lot` | 0.12 | 0.13.0 | 2026-10-21 | TisVlay pins 0.12. |
| `env_logger`, `log` | 0.11, 0.4 | 0.11.12 | 2026-11-20 | Replaced by `tracing` per the template. |
| `tokio`, `clap`, `serde`, `toml`, `futures-util` | | | | Take TisVlay's pins. |
| `uuid`, `futures` | | | | Unused in 0.4.0's sources. |

`zbus` is at 6.5.0; TisVlay holds 5 because `ksni` and Slint have not moved. CtrlAssist inherits that hold through `ksni`.

## Alternatives by goal

### Reading pads and creating virtual pads (evdev, uinput)

| Crate | Current | Last release | Notes |
|---|---|---|---|
| `evdev` | 0.13.2 | 2025-09-15 | The incumbent. Pure Rust, optional tokio. Used by InputPlumber (from git). |
| `evdevil` | 0.5.0 | 2026-06-18 | New since the legacy choice (first release 2025-06). Its README claims almost every evdev and uinput feature including force feedback and multitouch, hotplug support, few dependencies, and async through tokio or async-io. 0BSD. Small user base (about 3,200 downloads in 90 days against a million for `evdev`). Its tests create uinput devices and open them, the approach the plan intends for its own tests. |
| `input-linux` | 0.7.1 | 2024-08-19 | Thin, lower-level. Not updated in two years. |
| `evdev-rs` | 0.6.3 | 2025-09-14 | Bindings to the C libevdev. No reason to add a C dependency. |
| `uinput`, `uinput-tokio` | | 2018, 2024 | Stale. |

*For S2:* `evdev` and `evdevil`, compared on what the loop needs: grab, force-feedback upload and erase handling on the uinput side, non-blocking reads under epoll, dropped-event resync, and hotplug.

### Normalizing pad layouts

| Option | Notes |
|---|---|
| `gilrs` | Applies the bundled SDL_GameControllerDB and exposes named buttons and axes. Owns its own event loop and device handles. |
| SDL_GameControllerDB used directly | The database is a text file gilrs vendors; gilrs's parser is in its `mapping` module. No standalone Rust crate for it turned up in a crates.io search. Whether gilrs's parser is usable without its event loop is unverified. |
| `sdl3` | 0.22.1 (2026-11-09), bindings to the C library. Would bring SDL's own mapping and HIDAPI drivers, at the price of a large C dependency inside the input loop. |

*For S2:* gilrs as is; direct evdev plus the mapping database (through gilrs's parser if it can be reused, or a small parser of the same format).

### HID-level virtual pads (uhid) and controller emulation

| Crate | Current | Last release | Notes |
|---|---|---|---|
| `uhid-virt` | 0.0.8 | 2025-01-01 | Thin wrapper over `/dev/uhid`. Repository had merges in 2026-03. Used by InputPlumber. MIT OR Apache-2.0. The `archive/uhid` tag in this repository added it but never used it. |
| `uhidrs-sys` | 1.0.4 | 2025-01-01 | The FFI layer under `uhid-virt`. |
| `tokio-linux-uhid`, `uhid-fs`, `uhid-sys` | | 2019 | Stale. |
| `hidapi` | 2.6.7 | 2026-08-27 | Host-side hidraw access (reading a real pad's reports and descriptor). MIT. Used by InputPlumber. |
| `async-hid` | 0.5.3 | 2026-06-11 | Async host-side HID. |
| `hidreport`, `hut` | 0.6.0 | 2026-01, 2026-10 | HID report descriptor parser and usage tables, from the hidutils project. Useful for inspecting a captured descriptor. |
| `dualsense-rs`, `dualsense-controller`, `mdrv-ds-core` | 0.6.0, 0.2.0, 0.0.1 | 2024-03, 2026-07, 2026-09 | Host-side DualSense protocol crates: they talk *to* a real pad. None emulates one, but their report layouts and checksum handling are the same data an emulator needs. All small and young. |
| `vusb`, InputPlumber's `virtual-usb` | 0.2.1 | 2026-11-05 | Virtual USB devices over USB/IP. A level below uhid. Noted because InputPlumber depends on such a crate; why it needs USB-level emulation on top of uhid is unverified and worth learning before S5 concludes. |

*For S5:* `uhid-virt` for the uhid side, `hidapi` to capture a real DualSense's descriptor and reports, `hidreport` to read them. The emulation logic itself has to be written.

### Hotplug

| Option | Notes |
|---|---|
| `udev` (+ `tokio-udev` or its `mio` feature) | The incumbent; gives device properties and parent traversal, which legacy used to find a pad's hidraw siblings. Links libudev. InputPlumber uses it with mio. |
| `evdevil`'s hotplug | Built in; mechanism unverified. |
| `inotify` on `/dev/input` | 0.11.5 (2026-08-14). No C dependency, but no device properties. |
| Raw netlink uevents | `netlink-sys` 0.9.0 (2026-11-15), already in TisVlay's tree through rtnetlink. Pure Rust, but udev's properties (the stable identifiers the plan wants) are added by udevd, so they are only on udev's own netlink group, not the kernel's. |

*For S1 and S2:* `udev` is the default; the alternatives matter only if libudev becomes a packaging problem.

### The loop itself

`mio` 1.2.5, `polling` 3.11.0, `rustix` 1.1.6 and `nix` 0.32.0 all expose epoll and are current. TisVlay already has `rustix`. InputPlumber uses `mio`. Tokio's `AsyncFd` is the other route, at the cost of putting the input path on a runtime.

*For S2:* a dedicated thread on `rustix` or `mio`; decide there.

### Terminal UI

`ratatui` 0.30.2 (2026-06-19, MSRV 1.88) with `crossterm` 0.29.0. InputPlumber ships a ratatui interface, which is a working example of a TUI for this kind of daemon. The `archive/feature/tui` tag holds this project's earlier draft.

### Graph structure

`petgraph` 0.8.3 and `daggy` 0.9.0 are current. A routing graph of a few nodes, validated once when a profile loads and then walked on every event, likely wants its own flat representation for the hot path; petgraph is a candidate for validation (cycle detection, topological order) only.

*For S4:* hand-rolled executor; petgraph optional for validation.

### Control plane

`zenoh` 1.10.2 (2026-10-28), EPL-2.0 OR Apache-2.0; TisVlay pins `1.10` with only the Unix-socket transport enabled.

## Prior art

### InputPlumber

Read from its manifest and repository metadata on 2026-11-21 (version 0.85.0, pushed the day before, about 620 stars).

- **License: GPL-3.0-or-later.** CtrlAssist is Apache-2.0, so no code or data files can be copied from it. Reading it to learn how the kernel interfaces behave, and talking to it over its D-Bus API, are both fine.
- **Shape:** a system service with a system-bus D-Bus interface, polkit actions and rules, udev rules and hwdb entries, and a suspend hook unit. It ships device definitions, capability maps and profiles as YAML.
- **Stack:** `evdev` (git revision), `hidapi`, `uhid-virt`, `udev` with `mio`, a virtual-USB crate, `zbus`, `ratatui`, `tokio`.
- **What it does that overlaps:** composes source pads into virtual pads with selectable target identities (DualSense among them), and hides the sources. The plan review's fact-check read its DualSense target as uhid-based and its hiding as runtime udev rules plus a grab; this survey did not re-read those sources.
- **What it lacks that CtrlAssist is for:** nothing in its manifest or packaging suggests merging two people's pads with assist semantics; that was not checked in its sources.

It is preinstalled on Bazzite and some handheld distributions (unverified; from the plan review). Where it runs, it already owns hiding and virtual pads, so CtrlAssist must either cooperate with it or stay out of its way. That is a question for S1.

### SDL and Steam Input

Not surveyed beyond what the plan review established: SDL reads PlayStation pads through hidraw by default and applies mappings by the pad's ID; Valve's udev rules grant uinput and hidraw access but not uhid. S1 and S5 test both directly.

### Handhelds

Not surveyed. How a Steam Deck or similar presents its built-in controller, and what sits between it and games, remains open and is outside the first release's verified environment.

## Candidates handed to the spikes

| Spike | Candidates |
|---|---|
| Hiding, permissions, identification | `udev` for identifiers and sibling nodes; grab through `evdev` or `evdevil`; InputPlumber's approach as the privileged reference; its D-Bus API as the delegation route |
| Virtual pad identity | `evdev` or `evdevil` for the uinput pad; `uhid-virt` for the uhid pad; `hidapi` and `hidreport` to capture and read a real DualSense |
| Input loop | `evdev` against `evdevil`; gilrs against direct reads plus the mapping database; `rustix` or `mio` for epoll |
| Routing graph | No dependencies; petgraph optional for profile validation |
| zenoh beside the loop | `zenoh` at TisVlay's pin and feature set |

## Gaps

- Whether gilrs's mapping parser can be used without its event loop.
- What `evdev`'s unreleased commits contain that InputPlumber needs.
- Why InputPlumber uses virtual USB in addition to uhid.
- `evdevil`'s hotplug mechanism and its force-feedback API on the uinput side.
- Steam Input's and handhelds' behavior, left to the spikes.
