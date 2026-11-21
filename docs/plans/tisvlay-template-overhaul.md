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

**Repository.** Transfer `ruffsl/CtrlAssist` to `CtrlAssist/CtrlAssist` (stars, issues, releases and LFS objects carry over; old URLs redirect). Do it first, since CI checks, environments and trusted publishing are keyed to the owner. The rewrite lands on `main` in the same repo: one commit removes the legacy tree, and `v0.4.0` plus a `legacy/0.4` branch keep it reachable.

**App ID** is `io.github.ctrlassist.CtrlAssist`.

**Packaging channels:** deb, rpm, a binary tarball, and a Nix package with a NixOS module, through TisVlay's pipeline (`cargo auditable zigbuild --profile dist`, `cargo nfpm`, Sigstore attestations, draft GitHub release). Closes #9.

**A user-level daemon, never a system one.** The devices belong to the seat user and Steam's config is in their home. The privileged parts are static: udev rules that the native packages install (uinput access, and whatever spike S1 finds for hiding). There is no root process.

**Daemon and thin clients, as separate processes of one binary.** `ctrlassist service daemon` owns the devices and the routing graph. The CLI, tray and GUI are clients. Reasons to split rather than run one process: a GUI toolkit shouldn't share a process with the input loop; a tray or GUI crash shouldn't drop the virtual pad mid-game; Steam Deck game mode has no tray; and the engine must outlive a settings window.

**Flatpak is a second-tier channel.** A Flatpak can't install a system service or udev rules, but it can run the same user-level processes inside its sandbox. It gets whatever hiding works unprivileged (today: the Steam blacklist), as it does now. It exists because SteamOS and Bazzite are immutable and are where this tool is most wanted (#13).

**The core abstraction is a routing graph.** Sources (physical pads), sinks (virtual pads) and transform nodes (mux and demux modes), with force feedback flowing backward along the same edges. It unifies mux, demux, the legacy cookbook's chained instances, and per-control splitting (#53), and it is what a config file describes (#6). Spike S4 settles the node API.

## Proposed, pending review

**P4. `ctrlassist` on crates.io becomes the client SDK;** the binary lives in `ctrlassist-cli`, as in TisVlay. `cargo install` is no longer a supported install path, because a daemon needs a unit file and udev rules cargo can't install. 0.4.0 stays as the last binary release.

**P6. zenoh for the control plane,** as in TisVlay. Performance work belongs to the data path, which is internal to the daemon (ground rule 5); zenoh carries only control, state and, off the critical path, sampled pad state for visualizers. The one thing to confirm is that a zenoh session living in the same process leaves the input thread undisturbed, which S3 checks. No alternative IPC is spiked.

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

## Legacy features, to curate

The rewrite targets similar functionality, not identical. Each row needs a keep, rework or drop from the maintainer; the third column is a proposal.

| Legacy feature | What it did | Proposal |
|---|---|---|
| Mux: priority | Assist overrides primary per control when active | Keep; the reference behavior |
| Mux: average | Blend both pads' axes, OR the buttons | Keep |
| Mux: toggle | Assist's Mode button switches the active pad, with a resync | Keep; the Mode-button binding becomes configurable |
| Demux: unicast | Route one pad to one of N virtual pads, cycled by Mode | Keep as a graph node |
| Demux: multicast | Copy one pad to N virtual pads | Keep as a graph node (a fan-out edge may make it trivial) |
| Chained instances | Shell scripts with `sleep 1` wiring demux outputs into mux inputs | Replace with one graph in one daemon |
| Hide: steam | Edit `controller_blacklist` in Steam's `config.vdf`; needs a Steam restart | Decide after S1 |
| Hide: system | chmod device nodes; needs root | Replace with whatever S1 finds; no `sudo` path |
| Spoof | Virtual pad copies a physical pad's name and USB IDs | Keep; S1 checks how games and Steam treat it |
| Rumble routing | To primary, assist, both, active, or none | Keep, as feedback edges |
| Tray | Pick pads and options, start and stop, notifications | Keep as a client |
| Saved settings | Last-used tray choices in `config.toml` | Replace with daemon-owned profiles describing a graph |
| Per-control split (#53) | Not implemented | Falls out of P5; scope to decide |

## Spikes

Each is an independent workspace under `spikes/`, measured on the maintainer's hardware, and ends in `docs/` as a finding. The legacy 0.4.0 binary is the baseline where a comparison applies.

### S1. Hiding and permissions

*Question:* how does an unprivileged user daemon hide a physical pad from games, and what must the packages install?

*Method:* for each mechanism, record what Steam Input, SDL (evdev and HIDAPI drivers), Proton and a native evdev game still see:

- `EVIOCGRAB` on the evdev node (expected: hides from evdev and joydev readers, not from hidraw readers; needs no root; unverified).
- Steam's `controller_blacklist` (the legacy path).
- A udev rule plus group or ACL on the event and hidraw nodes (#19).
- Any others the survey turns up, including how InputPlumber does it.

Also record what grants `/dev/uinput` access on Bazzite, SteamOS, Fedora, Ubuntu and NixOS, natively and inside a Flatpak, since the legacy README never says and something on the tested systems already does it.

*Feeds:* the hide and spoof rows above, the packages' udev rules.

### S2. The input loop

*Question:* what does the data path look like, and is gilrs in it?

*Method:* a single thread blocked in epoll over the pads' evdev fds and the virtual pads' uinput fds (which deliver force-feedback requests), with no periodic wakeups. Fake physical pads are created through uinput so the spike runs without hardware. Measure against legacy: latency from source event to sink event (median and tail), CPU at a sustained high event rate from two pads, and wakeups per second when idle.

*The risk to resolve:* gilrs brings the SDL controller mapping database, which normalizes pads whose buttons and axes are laid out oddly. Reading evdev directly gives up gilrs's event loop (which a grab from S1 would starve anyway) and with it that normalization, unless the mapping is applied separately. The spike decides between gilrs as is, direct evdev plus the SDL mappings, or direct evdev alone.

*Feeds:* the device crate's design, #10, and whether S1's grab is usable.

### S3. zenoh beside the input loop

A check, not a comparison: the data path never touches zenoh, so this only confirms that sharing a process with it is harmless.

*Method:* S2's loop inside a daemon that also runs a zenoh session on a Unix socket in `$XDG_RUNTIME_DIR`, publishing state and serving a few rpcs in TisVlay's style, with one idle client attached. Re-run S2's measurements with the input thread isolated from zenoh's runtime threads, and record the daemon's idle wakeups and resident memory. Also prototype the visualizer feed: pad state sampled off the input thread and published only while something subscribes, so a GUI, tray or TUI can mirror a pad without sitting between hardware and game.

*Verdict:* zenoh stays if the input thread's numbers match S2's within measurement noise. If they don't, the finding says what isolates them (thread placement, a separate runtime) before any other IPC is considered.

*Feeds:* P6, and the daemon's threading model.

### S4. The routing graph

*Question:* what is the node API, and does one model express every row of the feature table?

*Method:* a pure crate, no device or async dependencies. Write scenario tests first for priority, average, toggle, unicast, multicast, a chained demux-into-mux setup, and rumble routing for each, then the nodes and an executor that make them pass. Property tests cover the invariants (for example: priority never forwards a primary button while assist holds it; outputs stay in range; a toggle switch leaves the sink consistent with the newly active pad). Benchmark a step of the executor to confirm it adds nothing measurable next to S2's numbers.

*Feeds:* the core crate, and the profile format.

S1 and S4 are independent and can run in parallel. S3 builds on S2.

## Steps

1. **Move the repository.** Maintainer: open a tracking issue, transfer the repo, update the local remote, enable private vulnerability reporting.
2. **Clean slate and scaffold.** On this branch:
   - `chore: remove the legacy tree` (after creating `legacy/0.4`), keeping `LICENSE`, the artwork and this plan.
   - `build: workspace scaffold from the TisVlay template`: exact-pinned toolchain, flake dev shell, `justfile`, `prek.toml`, `.config/`, workspace lint table and profiles, an empty `crates/`, and `spikes/` excluded from the workspace.
   - `ci: pipeline from the TisVlay template`: `ci.yml`, `pr-title.yml`, `audit.yml`, dependabot; SHA-pinned.
   - `docs: contributing guide, conventions, plans convention, CLAUDE.md`, with the docs skeleton (`architecture.md` as a stub, `ideas/`, `concerns/`, `bugs/`).
   - Verify: `prek run -a` and CI green on the empty workspace.
3. **Spikes S1 to S4,** each a series of commits ending in its finding.
4. **Architecture.** Write `docs/architecture.md` from the findings: crates, processes, data flow, key space, profile format, packaging and permissions. Revise this plan's Proposed section into decisions, and replace step 5 with commit-sized build steps. Review before building.
5. **Build,** in the order the architecture sets. Expected shape: core, device, protocol, daemon, SDK, CLI, packaging and first release, tray, then the GUI under its own plan.

Open legacy issues that the rewrite doesn't close are moved to `docs/ideas/` in step 2.

## Deviations

- **Third draft.** The maintainer confirmed the user daemon with thin clients, Flatpak as a second tier, and the general graph, which moved to Decided. S3 shrank from an IPC comparison to a co-existence check, since performance matters on the data path and that never leaves the daemon.
- **Second draft.** The first draft (previous commit) was a phased, behavior-preserving migration: scaffolding around the existing crate, then a crate split, packaging, a daemon, a GUI, with CLI compatibility kept through an in-process daemon mode. The maintainer's review replaced it with a clean-slate rewrite planned architecture-first: compatibility isn't wanted, piecemeal migration invites churn, and the performance and process-layout questions need spikes before the crates are drawn.
