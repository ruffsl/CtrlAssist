# Rewrite CtrlAssist on the TisVlay template

Status: **draft, awaiting maintainer review.** Nothing below is implemented.

## Goal

Rewrite CtrlAssist from a clean slate, using [TisVlay](https://github.com/TisVlay/TisVlay) as the reference for project style, architecture and patterns, and move the repository to the `CtrlAssist` GitHub org.

The legacy code (0.4.0: one crate, 4,000 lines, no tests) is a reference for *what users could do*, not for how it was done and not as a compatibility target. The end state offers similar functionality; the CLI, config file, tray layout, app ID and crate layout are all free to change. We are pre-1.0.

Why a rewrite rather than a migration:

- **The open issues are architectural.** Hotplug (#8), packaging (#9), a GUI (#7), richer configuration (#6), latency (#10) and per-control splitting (#53) each want something the legacy shape lacks: a long-lived owner of the devices, a general routing model, a testable core, a release pipeline.
- **Nothing is tested,** so there is no behavior to preserve with confidence. A migration would churn every file twice: once to move it, once to redesign it.
- **A generalization was already tried and stalled** (the `refactor-claud-opus-4.5` branch), for a reason this plan is built to avoid. See [Lessons from the earlier refactor](#lessons-from-the-earlier-refactor).

## Ground rules

1. **Architecture before build.** The crate split, process layout and data flow are designed up front from spike results and written into `docs/architecture.md`. No piecemeal migration.
2. **TisVlay is the template.** Tooling, lint policy, CI shape, docs layout, release flow and code conventions are copied from it and trimmed, not re-derived. Where CtrlAssist departs, the plan says why.
3. **No backward compatibility.** No config migration, no CLI aliases, no shims.
4. **Behavior is specified as tests before the engine exists.** The routing semantics live in a pure crate with no device access, and its tests are the spec.
5. **Input events never cross IPC.** The data path stays in one process; IPC carries control and state only.
6. **Spikes lean into each candidate's idioms.** A spike is an independent workspace under `spikes/<name>/`, written the way that technology wants to be used, and ends in a written finding with a keep-or-kill verdict. No shared abstraction across candidates.
7. **One campaign branch,** a commit per slice, each passing the gates; PRs are cut from the history when the shape is final.

## Decided

**Repository.** Transfer `ruffsl/CtrlAssist` to `CtrlAssist/CtrlAssist` (stars, issues, releases and LFS objects carry over; old URLs redirect). The maintainer does this out of band, and no local work waits on it: development runs on this branch through to completion, files are written against the final `CtrlAssist/CtrlAssist` URLs and app ID from the start, and everything that needs GitHub (CI runs, required checks, environments, trusted publishing, Read the Docs, Codecov) is switched on by the maintainer afterward. Until then the gates are the local ones, `prek run -a` and `just check`, and workflows are linted but not run. The rewrite lands on `main` in the same repo: one commit removes the legacy tree, and `v0.4.0` plus a `legacy/0.4` branch keep it reachable.

**App ID** is `io.github.ctrlassist.CtrlAssist`.

**Packaging channels:** deb, rpm, a binary tarball, and a Nix package with a NixOS module, through TisVlay's pipeline (`cargo auditable zigbuild --profile dist`, `cargo nfpm`, Sigstore attestations, draft GitHub release). Closes #9.

**A user-level daemon, never a system one.** The devices belong to the seat user and Steam's config is in their home. The privileged parts are static: udev rules that the native packages install (uinput access, and whatever spike S1 finds for hiding). There is no root process.

**Daemon and thin clients, as separate processes of one binary.** `ctrlassist service daemon` owns the devices and the routing graph. The CLI, tray and GUI are clients. Reasons to split rather than run one process: a GUI toolkit shouldn't share a process with the input loop; a tray or GUI crash shouldn't drop the virtual pad mid-game; Steam Deck game mode has no tray; and the engine must outlive a settings window.

**Flatpak is a second-tier channel.** A Flatpak can't install a system service or udev rules, but it can run the same user-level processes inside its sandbox. It gets whatever hiding works unprivileged (today: the Steam blacklist), as it does now. It exists because SteamOS and Bazzite are immutable and are where this tool is most wanted (#13).

**The core abstraction is a routing graph.** Sources (physical pads), sinks (virtual pads) and transform nodes (mux and demux modes), with force feedback flowing backward along the same edges. It unifies mux, demux, the legacy cookbook's chained instances, and per-control splitting (#53), and it is what a config file describes (#6). Spike S4 settles the node API.

**Crates.** `ctrlassist` on crates.io becomes the client SDK; the binary lives in `ctrlassist-cli`, as in TisVlay. `cargo install` is no longer a supported install path, because a daemon needs a unit file and udev rules cargo can't install. 0.4.0 stays as the last binary release.

**zenoh for the control plane,** as in TisVlay. Performance work belongs to the data path, which is internal to the daemon (ground rule 5); zenoh carries only control, state and, off the critical path, sampled pad state for visualizers. The one thing to confirm is that a zenoh session living in the same process leaves the input thread undisturbed, which S3 checks. No alternative IPC is spiked.

**Setups are presets first, graphs in config.** The CLI, tray and GUI offer named setups (assist, swap, split to N), each a small graph underneath. Arbitrary graphs can be written in a profile file from the first release; no client edits them visually.

**Disconnects hold and re-bind.** When a pad in a running setup drops, the virtual pad stays alive so the game never sees a disconnect, the missing pad's inputs go neutral, the other pad keeps working, and the same pad is re-bound when it returns. A notification says what happened.

**Pads are identified by a stable hardware ID,** with a fallback. Prefer the unique ID the kernel exposes (Bluetooth address, USB serial), then vendor and product plus port, then name. S1 records which of these are actually stable on the maintainer's pads.

**Virtual pad identity offers both** copying a connected pad and picking a built-in identity. How faithful an identity can be is S5's question: the legacy version never got PlayStation spoofing right.

**Clients are built in the order CLI, tray, GUI, TUI.** The GUI is Slint, following TisVlay. The TUI uses ratatui and comes last because TisVlay has none to pattern it on. Each can mirror live pad state from the daemon's sampled feed.

**The first release is daemon, CLI and tray,** which is what 0.4.0 users have today. The GUI and TUI follow as later minor releases.

**The first release is verified on NixOS with KDE,** the maintainer's host. The deb, rpm and tarball are built and container-tested but not hardware-verified; the Flatpak is not a first-release gate. Other environments are verified as people who run them report in.

**The event model isn't gamepad-shaped, but only gamepads are built.** See [Event model scope](#event-model-scope).

## Event model scope

The maintainer wants gamepads, wheels and HOTAS, and gyro, touchpad and LEDs carried by the core model, with wheels and HOTAS open to deferral if designing for them is costly. The proposal is to design for all of them and build only gamepads:

- **Controls are open-ended.** A control is identified by kind and index, with the standard gamepad buttons and axes as well-known names over that space rather than a closed enum. A HOTAS with 30 buttons and 8 axes is representable without a vocabulary change. Cost: the mux modes address controls generically instead of matching on a fixed enum, which S4's nodes need anyway to do per-control routing (#53).
- **Feedback carries the kernel's effect description,** not a rumble pair, so a wheel's constant-force and spring effects pass through unchanged even though only rumble is tested.
- **Motion, touch and LED events have reserved kinds** in the vocabulary. Whether any source or sink can produce or consume them depends on S5, since evdev and uinput carry little of it.

What is deferred is implementation and verification on real wheel or HOTAS hardware, which the maintainer doesn't have on hand, not the shape of the wire. That avoids the ossification risk at the price of some generality in S4. If S4 shows the open-ended form costs more than this, the finding says so and the decision is revisited.

## Lessons from the earlier refactor

The `refactor-claud-opus-4.5` branch defined `CtrlEvent`, `EdgeState`, a `Node` trait with ports, and a `GraphExecutor`. Its own assessment records what went wrong:

- The executor was built but never used. `mux2` hand-wired `GilrsDriver → MuxNode → sink` and kept a separate force-feedback thread outside the graph, reproducing the legacy two-loop design beside the new abstractions.
- Force feedback had no place in the model until late, so toggle mode sent rumble to both pads.
- There were no tests to say whether the new path matched the old, so each step risked a silent regression and the work stalled.

What this plan takes from it:

- The event vocabulary (normalized axes, named buttons, forward input and reverse feedback) is sound and is the starting point for the core crate.
- Feedback is part of the graph from the first test, not an addition.
- The spec comes first: scenario tests in a pure crate, then nodes, then an executor that the tests run through, then devices. Nothing is hand-wired around the executor.
- The node API is designed for the hot path from the start (no per-event allocation, no `dyn Any` state), which spike S4 settles.

## Feature goals

The rewrite targets similar functionality, not identical. Curated with the maintainer:

| Legacy feature | What it did | In the rewrite |
|---|---|---|
| Mux: priority | Assist overrides primary per control when active | First-class preset |
| Mux: average | Blend both pads' axes, OR the buttons | First-class preset |
| Mux: toggle | Assist's Mode button switches the active pad, with a resync | First-class preset; the switch binding becomes configurable |
| Demux: unicast, multicast | Route one pad to one or all of N virtual pads | First-class preset, as graph nodes |
| Chained instances | Shell scripts with `sleep 1` wiring demux outputs into mux inputs | One graph in one daemon, written in a profile |
| Hide: steam | Edit `controller_blacklist` in Steam's `config.vdf`; needs a Steam restart | Decided after S1 |
| Hide: system | chmod device nodes; needs root | Whatever S1 finds; no `sudo` path |
| Spoof | Virtual pad copies a physical pad's name and USB IDs | Copy a pad or pick a built-in identity; depth decided after S5 |
| Rumble routing | To primary, assist, both, active, or none | Feedback edges |
| Tray | Pick pads and options, start and stop, notifications | A client of the daemon |
| Saved settings | Last-used tray choices in `config.toml` | Daemon-owned profiles describing a graph |
| Per-control split (#53) | Not implemented | Expressible in a profile; no preset in the first release |

## Ecosystem survey

The legacy dependency set was chosen a year ago and hasn't been revisited, and the Rust input ecosystem moves quickly. Before the spikes pick their building blocks, survey it and write the result to `docs/` as a finding:

- **Legacy dependencies:** for each (`gilrs`, `evdev`, `udev`, `ksni`, `notify-rust`, `ashpd`, `fs2`, `dirs`, `signal-hook`, `parking_lot`, `env_logger`), the current release, what changed since the pinned version (gilrs's changelog especially), maintenance state, and whether TisVlay already uses it or a replacement.
- **Alternatives by goal:** evdev and uinput access; hidraw and uhid; controller mapping databases (SDL's and others); HID report descriptors and ready-made controller emulation, PlayStation above all; device hotplug (udev bindings against pure-Rust netlink or inotify); epoll-style loops that sit well beside tokio; ratatui and its widget ecosystem; graph or dataflow crates worth borrowing from for S4.
- **Prior art:** how InputPlumber, Steam Input, SDL and similar tools solve hiding, identity and routing, and under which licenses, so the spikes test known approaches rather than rediscover them.

Each candidate records its release date and source so the survey can be re-checked later. The survey chooses nothing by itself: it gives each spike its candidate list, and a spike may compare two candidates where the choice isn't obvious.

## Spikes

Each is an independent workspace under `spikes/`, measured on the maintainer's hardware, and ends in `docs/` as a finding. The legacy 0.4.0 binary is the baseline where a comparison applies.

### S1. Hiding and permissions

*Question:* how does an unprivileged user daemon hide a physical pad from games, and what must the packages install?

*Method:* for each mechanism, record what Steam Input, SDL (evdev and HIDAPI drivers), Proton and a native evdev game still see:

- `EVIOCGRAB` on the evdev node (expected: hides from evdev and joydev readers, not from hidraw readers; needs no root; unverified).
- Steam's `controller_blacklist` (the legacy path).
- A udev rule plus group or ACL on the event and hidraw nodes (#19).
- Any others the survey turns up, including how InputPlumber does it.

Also record, on NixOS with KDE: what grants `/dev/uinput` access (the legacy README never says, and something on the tested systems already does it), and which identifiers stay stable for an Xbox pad and a DualShock or DualSense across reconnects over USB and Bluetooth. Other distributions and the Flatpak sandbox are surveyed from documentation only, not tested.

*Feeds:* the hide row above, pad identification, the packages' udev rules.

### S2. The input loop

*Question:* what does the data path look like, and is gilrs in it?

*Method:* a single thread blocked in epoll over the pads' evdev fds and the virtual pads' uinput fds (which deliver force-feedback requests), with no periodic wakeups. Fake physical pads are created through uinput so the spike runs without hardware. Measure against legacy: latency from source event to sink event (median and tail), CPU at a sustained high event rate from two pads, and wakeups per second when idle.

*The risk to resolve:* gilrs brings the SDL controller mapping database, which normalizes pads whose buttons and axes are laid out oddly. Reading evdev directly gives up gilrs's event loop (which a grab from S1 would starve anyway) and with it that normalization, unless the mapping is applied separately. The spike decides between gilrs as is, direct evdev plus the SDL mappings, or direct evdev alone.

*Feeds:* the device crate's design, #10, and whether S1's grab is usable.

### S3. zenoh beside the input loop

A check, not a comparison: the data path never touches zenoh, so this only confirms that sharing a process with it is harmless.

*Method:* S2's loop inside a daemon that also runs a zenoh session on a Unix socket in `$XDG_RUNTIME_DIR`, publishing state and serving a few rpcs in TisVlay's style, with one idle client attached. Re-run S2's measurements with the input thread isolated from zenoh's runtime threads, and record the daemon's idle wakeups and resident memory. Also prototype the visualizer feed: pad state sampled off the input thread and published only while something subscribes, so a GUI, tray or TUI can mirror a pad without sitting between hardware and game.

*Verdict:* zenoh stays if the input thread's numbers match S2's within measurement noise. If they don't, the finding says what isolates them (thread placement, a separate runtime) before any other IPC is considered.

*Feeds:* the daemon's threading model.

### S4. The routing graph

*Question:* what is the node API, and does one model express every row of the feature table?

*Method:* a pure crate, no device or async dependencies. Write scenario tests first for priority, average, toggle, unicast, multicast, a chained demux-into-mux setup, rumble routing for each, a pad dropping out and returning mid-setup, and one device that isn't gamepad-shaped (many buttons, extra axes, a non-rumble effect) to prove the open-ended vocabulary, then the nodes and an executor that make them pass. Property tests cover the invariants (for example: priority never forwards a primary button while assist holds it; outputs stay in range; a toggle switch leaves the sink consistent with the newly active pad). Benchmark a step of the executor to confirm it adds nothing measurable next to S2's numbers.

*Feeds:* the core crate, the profile format, and the event model scope.

### S5. Virtual pad identity

*Question:* what does it take for a virtual pad to pass as a specific controller, PlayStation above all?

*Background, unverified:* Steam and SDL read DualShock and DualSense pads through hidraw rather than evdev, so a uinput pad carrying Sony's IDs presents an evdev node with no HID device behind it. A convincing one would be emulated at the HID level through uhid, which is believed to be InputPlumber's approach.

*Method:* build one uinput pad with copied IDs and one uhid pad replaying a real DualSense's report descriptor, and record what Steam Input, SDL, Proton and a native game make of each: detection, button glyphs, rumble, and whether gyro, touchpad and LEDs come through. Do the same for an Xbox identity, where uinput may already be enough.

*Verdict:* the finding recommends uinput only, uhid for some identities, or uhid throughout, with the cost of each. The architecture keeps the sink type open until then.

*Feeds:* virtual pad identity, the reserved motion, touch and LED event kinds, and #5.

S1, S4 and S5 are independent and can run in parallel. S3 builds on S2.

## Steps

1. **Move the repository.** Maintainer, out of band and at any point: transfer the repo, update the local remote, enable private vulnerability reporting, then switch on what the workflows need. Not a prerequisite for any later step.
2. **Clean slate and scaffold.** On this branch:
   - `chore: remove the legacy tree` (after creating `legacy/0.4`), keeping `LICENSE`, the artwork and this plan.
   - `build: workspace scaffold from the TisVlay template`: exact-pinned toolchain, flake dev shell, `justfile`, `prek.toml`, `.config/`, workspace lint table and profiles, an empty `crates/`, and `spikes/` excluded from the workspace.
   - `ci: pipeline from the TisVlay template`: `ci.yml`, `pr-title.yml`, `audit.yml`, dependabot; SHA-pinned.
   - `docs: contributing guide, conventions, plans convention, CLAUDE.md`, with the docs skeleton (`architecture.md` as a stub, `ideas/`, `concerns/`, `bugs/`).
   - Verify: `prek run -a` green on the empty workspace; `actionlint` and `zizmor` clean on the workflows, which first run after the move.
3. **Ecosystem survey,** ending in its finding.
4. **Spikes S1 to S5,** each a series of commits ending in its finding.
5. **Architecture.** Write `docs/architecture.md` from the findings: crates, processes, data flow, key space, profile format, packaging and permissions. Replace step 6 with commit-sized build steps. Review before building.
6. **Build,** in the order the architecture sets. Expected shape: core, device, protocol, daemon, SDK, CLI, tray, packaging and the first release; then the GUI and the TUI, each under its own plan.

Open legacy issues that the rewrite doesn't close are moved to `docs/ideas/` in step 2.

## Deviations

- **Fifth draft.** Added the ecosystem survey ahead of the spikes, since the legacy dependencies are a year stale. The repository move no longer gates anything: the maintainer does it out of band and local work runs to completion without GitHub.
- **Fourth draft.** A round of maintainer questions settled the crate naming, zenoh, presets over a graph, disconnect handling, pad identification, the client order (adding a ratatui TUI), the first release's contents and its single verified environment. The feature table became goals. New: spike S5 on virtual pad identity, and the event model scope section answering whether to design for wheels and HOTAS now.
- **Third draft.** The maintainer confirmed the user daemon with thin clients, Flatpak as a second tier, and the general graph, which moved to Decided. S3 shrank from an IPC comparison to a co-existence check, since performance matters on the data path and that never leaves the daemon.
- **Second draft.** The first draft (previous commit) was a phased, behavior-preserving migration: scaffolding around the existing crate, then a crate split, packaging, a daemon, a GUI, with CLI compatibility kept through an in-process daemon mode. The maintainer's review replaced it with a clean-slate rewrite planned architecture-first: compatibility isn't wanted, piecemeal migration invites churn, and the performance and process-layout questions need spikes before the crates are drawn.
