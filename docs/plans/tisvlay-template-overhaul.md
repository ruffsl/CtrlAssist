# Rewrite CtrlAssist on the TisVlay template

## State

- **Status:** plan approved by the maintainer; step 1 done.
- **Next action:** step 2 (ecosystem survey), stopping when its finding is written.
- **Branch:** `feat/rewrite`, one linear campaign branch. Nothing is pushed.
- **Template source:** the maintainer's TisVlay checkout. Record the TisVlay commit copied from in each commit that copies from it.
- **Hardware on the development host:** Xbox and DualShock/DualSense pads, Steam. No wheel or HOTAS.
- **Legacy baseline:** after step 1 removes the tree, build 0.4.0 from a `git worktree` of `legacy/0.4`, which carries its own flake.
- **Working rules,** until step 1's `CLAUDE.md` carries them:
  - Commit coherent checkpoints with Conventional Commit subjects. No `Co-Authored-By` trailer. Never amend or force-push.
  - Run the gates that exist before committing.
  - Plan revisions are committed as they are made, so they read as diffs.
  - No spike numbers, step numbers or ground-rule numbers in code or durable docs.
  - No standalone shell scripts for tooling or spike harnesses: `just` recipes or Rust binaries.
  - Ask before launching agents, including for spikes this plan calls independent.
  - Stop for review each time a finding is written.

## Goal

Rewrite CtrlAssist from a clean slate, using [TisVlay](https://github.com/TisVlay/TisVlay) as the reference for project style, architecture and patterns, and move the repository to the `CtrlAssist` GitHub org.

The legacy code (0.4.0: one crate, 3,900 lines of Rust, no tests) is a reference for *what users could do*, not for how it was done and not as a compatibility target. The end state offers similar functionality; the CLI, config file, tray layout, app ID and crate layout are all free to change. We are pre-1.0.

Why a rewrite rather than a migration:

- **The open issues are architectural.** Packaging (#9), a GUI (#7), a config file for advanced multiplexing (#6), latency (#10), splitting one device into several (#53) and faithful PlayStation identity (#5) each want something the legacy shape lacks: a long-lived owner of the devices, a general routing model, a testable core, a release pipeline.
- **Nothing is tested,** so there is no behavior to preserve with confidence. A migration would churn every file twice: once to move it, once to redesign it.
- **A generalization was already tried and stalled** (the `refactor-claud-opus-4.5` branch). See [Lessons from the earlier refactor](#lessons-from-the-earlier-refactor).

This is a program plan. It fixes the decisions, the spikes and their exit criteria; the commit-sized build steps are written at step 4, from the spike findings. That and the other departures from the template's process are listed under [Departures from the template's process](#departures-from-the-templates-process).

## Terms

One word per thing, here and in everything written from this plan:

| Term | Meaning |
|---|---|
| pad | A physical controller. Not "controller" or "device", except in crate names. |
| virtual pad | A controller the daemon creates for games to read. |
| source, sink | A pad, or a virtual pad, as it appears in a graph. Input flows source to sink; feedback originates at a sink and flows to sources. |
| node | A transform between sources and sinks (a mux or demux mode). |
| graph | Sources, nodes, sinks and the edges between them. |
| profile | A saved graph with its settings, in a file the daemon owns. |
| preset | A built-in profile shape a client offers by name (assist, swap, split). |
| setup | A profile that is currently running. |
| hide | Keep games from seeing a pad. A grab is one mechanism for it. |
| identity | The name, IDs and, where emulated, the HID personality a virtual pad presents. "Spoofing" is the legacy word. |
| feedback | Anything flowing from game to pad: force feedback (rumble is one kind), LEDs, trigger effects. |

## Ground rules

1. **Architecture before build.** The crate split, process layout and data flow are designed up front from spike results and written into `docs/architecture.md`. No piecemeal migration.
2. **TisVlay is the template.** Tooling, lint policy, CI shape, docs layout, release flow and code conventions are copied from it and trimmed, not re-derived. Where CtrlAssist departs, the plan says why.
3. **No backward compatibility.** No config migration, no CLI aliases, no shims.
4. **Behavior is specified as tests before the engine exists.** The routing semantics live in a pure crate with no device access, and its tests are the spec.
5. **Input events never cross IPC.** The data path stays in one process; IPC carries control and state only.
6. **Spikes lean into each technology's idioms.** A spike is an independent workspace under `spikes/<name>/` with a README, written the way that technology wants to be used. Where a spike compares candidates, each gets its own artifact; no shared abstraction across them.
7. **Tooling arrives with its first use.** A gate, workflow or document lands in the commit that gives it something to govern, not ahead of it.
8. **One campaign branch,** a commit per slice, each passing the gates that exist. How it is cut into PRs is decided when the shape is final; a history that begins by deleting the legacy tree may well land as one merge.

## Decided

**Repository.** Transfer `ruffsl/CtrlAssist` to `CtrlAssist/CtrlAssist`: issues, stars and LFS objects carry over and old URLs redirect, provided no repository is later created under the old name. The maintainer does this out of band, and no local work waits on it: files are written against the final URLs and app ID from the start, and everything that needs GitHub (CI runs, required checks, environments, trusted publishing, Read the Docs, Codecov) is switched on afterward. The rewrite lands on `main` in the same repo: one commit removes the legacy tree, and `v0.4.0` plus a `legacy/0.4` branch keep it reachable.

**App ID** is `io.github.ctrlassist.CtrlAssist`.

**Packaging channels:** deb, rpm and an Arch package through TisVlay's pipeline unchanged (`cargo auditable zigbuild --profile dist`, `cargo nfpm`, Sigstore attestations, draft GitHub release), and a Nix package with a NixOS module. This covers what #9 asks for apart from a bare binary download and Flathub.

**A user-level daemon,** unlike TisVlay's system daemon: the pads belong to the seat user and Steam's config is in their home. The packages install static udev rules for what the daemon needs (uinput access, and uhid if S5 calls for it).

**Privilege is confined to a minimal helper, and only if needed.** The daemon never runs as root. If S1 finds that no unprivileged mechanism makes a pad absent to games, the native packages install a small privileged helper (socket-activated or a polkit action) that hides and restores pads and does nothing else. Channels that can't install it, the Flatpak above all, fall back to the unprivileged mechanisms.

**Daemon and thin clients, as separate processes of one binary.** `ctrlassist service daemon` owns the pads and the graph. The CLI, tray, GUI and TUI are clients. Reasons to split rather than run one process: a GUI toolkit shouldn't share a process with the input loop; a client crash shouldn't drop the virtual pad mid-game; a gamescope session may have no tray; and the engine must outlive a settings window.

**Flatpak is a second-tier channel.** It can run the same user-level processes inside its sandbox and gets whatever hiding works there. It matters because SteamOS and Bazzite are immutable, but it is not a first-release gate.

**The core abstraction is a routing graph** of sources, nodes and sinks, with feedback as part of the model from the first test. It unifies mux, demux, the legacy cookbook's chained instances and splitting one pad into several (#53), and it is what a profile describes (#6). Profiles are validated as acyclic.

**Crates.** `ctrlassist` on crates.io becomes the client SDK; the binary lives in `ctrlassist-cli`, as in TisVlay. `cargo install` is no longer a supported install path, because a daemon needs a unit file and udev rules cargo can't install.

**zenoh for the control plane,** as in TisVlay, over a Unix socket in `$XDG_RUNTIME_DIR`. The alternative is the session D-Bus through zbus, which is already in the tree for the tray and would give activation and name ownership for free. zenoh is chosen because TisVlay served D-Bus first and moved off it for type safety and API ergonomics, and because its protocol, SDK and state-watching patterns carry over directly. Performance work belongs to the data path, which never touches it (ground rule 5). S3 checks only that sharing a process with it is harmless.

**Presets first, graphs in profiles.** Clients offer presets; arbitrary graphs can be written in a profile file from the first release; no client edits them visually.

**Disconnects hold and re-bind.** When a pad in a setup drops, the virtual pad stays alive so the game never sees a disconnect, the missing pad's inputs go neutral, the other pad keeps working, and the same pad is re-bound and re-hidden when it returns. A notification says what happened.

**Virtual pad identity offers both** copying a pad and picking a built-in identity. How faithful an identity can be, and what a virtual pad presents by default, are S5's questions.

**Setups start manually, with optional auto-start.** A client starts a setup. A profile may also be marked to start by itself once the daemon is up and all of its pads are connected.

**Clients are built in the order CLI, tray, GUI, TUI.** The GUI is Slint, following TisVlay. The TUI uses ratatui and comes last because TisVlay has none to pattern it on; the `archive/feature/tui` tag holds an earlier draft worth reading first. Each can mirror live pad state, which the daemon publishes on change and only while something subscribes.

**The first release is daemon, CLI and tray,** matching the surfaces 0.4.0 has (a CLI and a tray). The GUI and TUI follow as later minor releases. It is verified on NixOS with KDE, the maintainer's host; the deb, rpm and Arch packages are built and container-tested but not hardware-verified.

**The event model isn't gamepad-shaped, but only gamepads are built.** See [Event model scope](#event-model-scope).

## Provisional, pending spikes

**Whether the privileged helper is needed,** and what it does. Games run as the same user as the daemon, so a group or ACL rule can't admit the daemon while excluding the game, which leaves a grab and Steam's blacklist as the unprivileged mechanisms, both partial. InputPlumber hides pads with runtime udev rules, which needs root. S1 decides between: unprivileged mechanisms are enough and no helper ships; the helper applies the hiding; or, where InputPlumber is present, hiding is delegated to it.

**User-facing names.** The engine keeps the legacy words (mux, demux, and their modes) in code and profiles. What clients call the presets and the two roles is open until the CLI is designed at step 4. The proposal on the table: roles "player" and "helper" in place of primary and assist; presets "assist" (priority), "blend" (average), "take turns" (toggle), "switch" (unicast) and "mirror" (multicast).

**Pads are identified by a stable hardware ID,** falling back to vendor and product plus port, then name. S1 records which identifiers are actually stable.

**Feedback's representation.** A uinput sink receives kernel force-feedback requests; a uhid sink would receive raw HID output reports. Until S5 reports, feedback is an open set of kinds (force-feedback effect, HID output report, LED), not the kernel effect alone.

## Constraints the design must respect

Facts about the platform that the plan review surfaced. The spikes test the ones in doubt; the architecture must answer all of them.

- **Hidden means absent, not silent.** A grabbed evdev node still enumerates, so a game may list a dead pad and give it player 1, and hidraw has no grab at all. Hiding is judged by whether the consumer still lists the pad.
- **Hiding must survive a crash.** Legacy restored permissions and Steam's config on `Drop`, which a kill or power loss skips, leaving pads hidden. Prefer mechanisms the kernel undoes when the process exits. Any persistent change is recorded before it is made, reconciled when the daemon starts, and undone when its unit stops.
- **Force feedback is a blocking request protocol.** A game's effect upload blocks until the daemon answers, so a stalled input thread stalls the game. Effect slots are limited and per pad, effect IDs are per pad, a target may reject an upload, effects die with the handle that uploaded them, and gain and autocenter arrive separately. Fan-out to several pads needs a policy for what the game is told on partial failure; a demux creates fan-in, several games driving one pad. Legacy also let rumble go to a pad other than the one providing input, so feedback routing is not simply the input edges reversed.
- **A virtual pad's capabilities and identity are fixed at creation.** Changing them means destroying and recreating it, which the game sees as a disconnect. So a sink is created from what the profile declares, with a copied identity cached by hardware ID rather than read from a pad that may be absent. Changes to a running setup are either live (the sink is untouched) or disruptive, and clients say which.
- **Merging different pads needs a common layout.** Raw evdev layouts differ between drivers (which axis is the right stick, hats against d-pad buttons, analog against digital triggers, ranges), and a sink that copies a real pad's IDs gets that pad's mapping applied by SDL, so its layout must match. Events are processed in whole report frames, and a dropped-events marker forces a resync.
- **The daemon must not consume its own output.** Its virtual pads are excluded as sources by a marker. Other software's virtual pads (Steam Input's, InputPlumber's) are opt-in, since on a handheld one of them may be the only usable source and Steam's may itself wrap a CtrlAssist sink.
- **The development host masks permission questions.** Its `/dev/uinput` and `/dev/uhid` are accessible only because Steam's and Sunshine's udev rules are installed. Permission findings come from a clean NixOS VM.

## Event model scope

The core model carries gamepads, wheels and HOTAS, and gyro, touchpad and LEDs. All are designed for; only gamepads are built.

- **Controls are open-ended.** A control is identified by kind and index, with the standard gamepad buttons and axes as well-known names over that space rather than a closed enum. A HOTAS with 30 buttons and 8 axes is representable without a vocabulary change. The mux nodes need generic control addressing anyway to route per control (#53). Well-known names are what make two different pads mergeable; controls outside them pass through unmerged.
- **Feedback kinds are open** (see Provisional), so a wheel's constant-force and spring effects pass through even though only rumble is tested.
- **Motion, touch and LED kinds are reserved.** Whether any source or sink can carry them depends on S5.

Deferred: implementation and verification on wheel or HOTAS hardware, which isn't on hand. If S4 shows the open-ended form costs more than this, its finding says so and the decision is revisited.

## Lessons from the earlier refactor

The `refactor-claud-opus-4.5` branch defined `CtrlEvent`, `EdgeState`, a `Node` trait with ports, and a `GraphExecutor`, with ten unit tests over the core. Its mid-course assessment found the executor built but bypassed by hand-wired code; by the branch's end `mux2` does run input through the executor, but force feedback is still a separate thread outside the graph, a "hybrid" by its own description. Toggle mode sent rumble to both pads until a test pinned it.

The maintainer's account of why it stalled: nothing compared the new path's behavior with the legacy one, so features couldn't be ported without risking silent regressions.

What this plan takes from it:

- The event vocabulary (normalized axes, named buttons, forward input and reverse feedback) is the starting point for the core crate.
- Feedback is in the graph from the first test, with its request-and-reply shape, not a thread beside it.
- The spec comes first: scenario tests in a pure crate, then nodes, then an executor the tests run through, then devices. The behaviors are curated here rather than inherited.
- The node API is designed for the hot path (the branch's returns a `Vec` per event and holds state as `dyn Any`).

## Feature goals

The rewrite targets similar functionality, not identical.

| Legacy feature | What it did | In the rewrite |
|---|---|---|
| Mux: priority | Assist overrides primary while active: per button, per stick as a pair; triggers take the larger value | Preset |
| Mux: average | Blend both pads' axes, OR the buttons | Preset |
| Mux: toggle | Assist's Mode button switches the active pad, with a resync | Preset; the switch binding becomes configurable |
| Demux: unicast | Route one pad to the active one of N virtual pads, cycled by the Mode button, neutralizing the one left | Preset, as a node |
| Demux: multicast | Copy one pad to all N virtual pads | Preset, as a node |
| Chained instances | Shell scripts with `sleep 1` wiring demux outputs into mux inputs | One graph in one daemon, written in a profile |
| Hide: steam | Edit `controller_blacklist` in Steam's `config.vdf`; needs a Steam restart | Decided after S1 |
| Hide: system | chmod device nodes; needs root | Decided after S1; no `sudo ctrlassist` |
| Spoof | Virtual pad copies a pad's name and USB IDs | Identity: copy a pad or pick a built-in one; depth decided after S5 |
| Rumble routing | Mux: to primary, assist, both, active, or none. Demux: from the active virtual pad, or none | Feedback routing in the graph |
| Tray | Pick pads and options, start and stop, notifications | A client of the daemon |
| Saved settings | Last-used tray choices in `config.toml` | Profiles |
| Splitting one pad (#53) | Not implemented | Expressible in a profile; no preset in the first release |

## Ecosystem survey

The legacy dependencies were chosen a year ago, and the Rust input ecosystem moves quickly. Before the spikes pick their building blocks, survey it and write `docs/plans/findings/ecosystem.md`:

- **Legacy dependencies** (`gilrs`, `evdev`, `udev`, `ksni`, `notify-rust`, `ashpd`, `fs2`, `dirs`, `signal-hook`, `parking_lot`, `env_logger`, and the common ones TisVlay already pins): the current release, what changed since the pinned version (gilrs's changelog especially), maintenance state, and what TisVlay uses in that role.
- **Alternatives by goal:** evdev and uinput access; hidraw and uhid (the `archive/uhid` tag tried `uhid-virt` and `hidapi`); controller mapping databases; HID report descriptors and ready-made controller emulation, PlayStation above all; hotplug (udev bindings against pure-Rust netlink); epoll-style loops beside tokio; ratatui; graph or dataflow crates worth borrowing from.
- **Prior art:** how InputPlumber, Steam Input and SDL handle hiding, identity and routing, under which licenses, and how handhelds with built-in controllers present them.

Each entry records its source and date. The survey chooses nothing: it gives each spike its candidates. Done when every bullet above has an entry and each spike below has a candidate list.

## Spikes

Each spike is a workspace under `spikes/<name>/`, run on the development host, with the maintainer asked only for physical actions such as pressing buttons. Each ends in `docs/plans/findings/<name>.md` holding a results table and a verdict, and work stops there for review. At step 4 the findings are folded into `docs/architecture.md` (hardware caveats into `docs/concerns/`) and the findings directory is deleted. Spike code is deleted in the commit that lands its replacement in `crates/`, except S4, whose tests and nodes are promoted to become the core crate.

Order: the survey, then S1 and S5, then S2, then S4, then S3. S5 precedes S4 because the sink type decides what feedback looks like; S1 precedes S2 because a grab decides who may read the pad.

### S1. Hiding, permissions and identification

*Question:* can an unprivileged daemon make a pad absent to games, and what must the packages install?

- [ ] For each mechanism (evdev grab; Steam's blacklist; SDL's ignore-devices hint per launch; runtime udev rules as InputPlumber uses, applied by a privileged helper; others from the survey) and each consumer (Steam Input, a native SDL game, a Proton game, a raw evdev reader), record: absent, present but silent, or visible.
- [ ] For each mechanism, record what remains after the daemon is killed without cleanup, and after the pad reconnects.
- [ ] Record Steam's behavior by start order (daemon first, Steam first) and whether a change needs a Steam restart.
- [ ] In a clean NixOS VM with no Steam or Sunshine rules: what a seat user can open among `/dev/uinput`, `/dev/uhid` and the pads' event and hidraw nodes.
- [ ] Record which identifiers stay stable for an Xbox pad and a DualShock or DualSense across reconnects, over USB and Bluetooth.
- [ ] Write the finding.

*Verdict:* a mechanism qualifies as the default only if it reaches "absent" for Steam Input and for a native SDL game, and leaves nothing behind after a kill. If no unprivileged mechanism qualifies, the helper ships, and the finding specifies the smallest interface it needs and what it costs to package.

*Feeds:* the provisional helper decision, pad identification, both hide rows, the udev rules.

### S5. Virtual pad identity

*Question:* what does it take for a virtual pad to pass as a specific pad, PlayStation above all (#5)?

*Background:* SDL reads DualShock and DualSense pads through hidraw by default, and Steam is believed to as well. A uinput pad carrying Sony's IDs has no HID device behind it; a uhid pad does, and the kernel's PlayStation driver can bind to it. InputPlumber emulates a DualSense this way.

- [ ] Build a uinput virtual pad with a DualSense's copied IDs, and a uhid one replaying a real DualSense's report descriptor.
- [ ] For each, record what Steam Input, a native SDL game and a Proton game report: detected as what, button glyphs, layout correctness, rumble, and whether gyro, touchpad and LEDs pass.
- [ ] Repeat for an Xbox identity.
- [ ] Record what feedback a uhid sink delivers (output reports) and what it needs installed (`/dev/uhid` access).
- [ ] Write the finding.

*Verdict:* uinput only, uhid for named identities, or uhid throughout, and which identity is the default when a profile names none. uhid is kept for an identity only if it is detected as that pad with correct glyphs and working rumble where uinput is not.

*Feeds:* identity, feedback's representation, the reserved motion, touch and LED kinds, the udev rules.

### S2. The input loop

*Question:* what does the data path look like, and what reads the pads?

- [ ] Build fake pads through uinput so the loop runs without hardware, and a harness that timestamps an event entering a fake pad and leaving a virtual pad.
- [ ] Build the loop: one thread blocked in epoll over the pads and the virtual pads' feedback requests, processing whole report frames, resyncing on dropped events, answering feedback uploads.
- [ ] Decide the reader: gilrs as is; direct evdev with a mapping database applied; or direct evdev alone for pads already in a standard layout. Test the first two with one pad that needs a mapping.
- [ ] Measure the loop and the legacy 0.4.0 binary, on an idle host and with every core loaded: latency median and tail, CPU at a sustained high event rate from two pads, and wakeups while idle (`perf stat` and the thread's context-switch counters).
- [ ] Record whether the input thread needs real-time scheduling to hold its tail under load, and what granting that costs to package.
- [ ] Exercise suspend and resume and a Bluetooth drop, and record the race between a returning pad being re-hidden and a game seeing it.
- [ ] Write the finding.

*Verdict:* the loop is kept if it makes no wakeups while idle and is no worse than legacy on latency tail and CPU in every measured condition. The reader is the cheapest option that merges two different pads correctly.

*Feeds:* the device crate, the daemon's threading, #10.

### S4. The routing graph

*Question:* what is the node API, and does one model express every row of the feature table?

- [ ] In a pure crate with no device or async dependencies, write scenario tests first: each mux and demux mode; a chained demux into mux; feedback for each (upload, play, stop, erase, gain, with replies); rumble routed to a pad other than the one providing input; a target rejecting an upload; two sinks driving one pad; a pad dropping out and returning; and one source that isn't gamepad-shaped.
- [ ] Write the nodes and an executor that make them pass, with nothing wired around the executor.
- [ ] Add property tests for the invariants (priority never forwards a primary button while assist holds it; outputs stay in range; a switch leaves the sink consistent with the newly active pad; no effect outlives its erase).
- [ ] Benchmark one executor step and compare it with S2's per-event cost.
- [ ] Draft the profile format for three presets and one hand-written graph.
- [ ] Write the finding.

*Verdict:* the model is kept if every scenario passes through the executor without a special case, a step allocates nothing, and its cost is below S2's measurement noise. The open-ended vocabulary is kept if the non-gamepad scenario needed no change to node code.

*Feeds:* the core crate (by promotion), the profile format, the event model scope.

### S3. zenoh beside the input loop

A check, not a comparison.

- [ ] Put S2's loop in a daemon that also runs a zenoh session on a Unix socket, publishing state and serving a few rpcs in TisVlay's style, with one idle client attached and the input thread kept off zenoh's runtime.
- [ ] Re-run S2's measurements; record the daemon's idle wakeups and resident memory, and a one-shot CLI call's time from start to reply.
- [ ] Prototype the pad-state mirror: published on change, only while subscribed.
- [ ] Write the finding.

*Verdict:* zenoh stays if the input thread's latency and wakeup numbers fall within the run-to-run spread S2 recorded. If not, the finding says what isolates them before any other IPC is considered.

*Feeds:* the daemon's threading model.

## Steps

1. **Clean slate and minimal scaffold.**
   - `chore: remove the legacy tree`, after creating `legacy/0.4`; keeps `LICENSE`, the artwork and this plan.
   - `build: toolchain, dev shell and file checks from the TisVlay template`: exact-pinned `rust-toolchain.toml`, the flake dev shell, a `justfile` and `prek.toml` with the file checks and `typos` (allowing the legacy branch name), `docs/plans/README.md` listing this plan, and a `CLAUDE.md` carrying the working rules and terms above. No cargo gates yet: a workspace with no members fails them.
   - Verify: `prek run -a` green; `nix develop -c just` lists the recipes.
2. **Ecosystem survey.** Verify: its done-condition above.
3. **Spikes,** in the order above, each with its checklist. Verify: each finding's results table and verdict.
4. **Architecture.** Write `docs/architecture.md` from the findings: crates, processes, threading, data flow, key space, profile format, permissions and packaging. Settle the Provisional section. Triage the open legacy issues into closed-by-design or `docs/ideas/` entries. Replace step 5 with commit-sized build steps, each with its verification. Review with the three lenses (design, correctness of claims, maintainability) before building.
5. **Build,** in the order the architecture sets. Expected shape: core (promoted from S4, bringing the workspace lint table, the cargo gates and `ci.yml`), device, protocol, daemon, SDK, CLI, tray, then packaging (bringing the release workflows, licensing checks, contributing guide and book) and the first release; then the GUI and the TUI, each under its own plan.
6. **Retire this plan** into `docs/` per the plans convention, in the last commit before the first release.

The repository move is the maintainer's, out of band, and gates none of these.

## Out of scope for the first release

Each becomes a `docs/ideas/` entry at step 4, with the trigger that would bring it in:

- The GUI and TUI (next minor releases).
- Wheel and HOTAS sources, and non-rumble force feedback (someone with the hardware to verify).
- Gyro, touchpad, LED and trigger-effect passthrough (S5 keeping uhid, then demand).
- A preset for splitting one pad (#53), and button debounce, which that issue also asks for.
- A visual graph editor.
- Flatpak and Flathub (a maintainer or contributor on SteamOS or Bazzite, #13).
- A bare binary tarball (a request from a distro none of the packages serve).
- Hardware verification of the deb, rpm and Arch packages.
- Keyboard and mouse as sources or sinks.

## Departures from the template's process

- **Build steps are deferred.** The plans convention asks for commit-sized steps with verification up front; here they are written at step 4, because the spikes decide the crates.
- **One plan for several spikes.** TisVlay gave its spike its own plan because it compared candidate technologies. These spikes answer different questions about one design, so they share this plan and get a finding each.
- **No issue or draft PR yet.** The contributing guide starts from an agreed issue and a draft PR. The maintainer is working locally until the repository moves, so the plan's history is this branch.
- **A user daemon,** where TisVlay's is a system service with polkit rules.

## Deviations

None yet. The drafting history is in git.
