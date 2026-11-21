# Rewrite CtrlAssist on the TisVlay template

## State

- **Status:** steps 1 to 4 done and approved by the maintainer. Step 5, the build, is under way: build steps 1 to 9 are done. The maintainer has read the hiding service's root code, and it hid a real DualSense from a running Steam and restored it (2026-11-21; SDL alone was not checked separately).
- **Next action:** vendor the `evdev` crate with its `phys` fix, then build step 10, hotplug. Build steps 7 to 9 (the pad reader, uinput virtual pads, the input thread) are done.
- **Overnight, 2026-11-21:** the maintainer asked for the marked stops at build steps 11, 12 and 16 to be built through, with each decision made there recorded under "For review" below. Also decided: the controller database is a pinned snapshot in the repository plus a user override file; the `evdev` fix is vendored and soaked here before any upstream PR; the generic identity keeps 0.4.0's numbers until the release.
- **Hardware checks owed:** `just forward-pad <node>` and `just forward-pad <node> --xbox` (SDL and Steam list each identity); `just bench`; rumble from a game reaching a real pad through a mux, which needs the daemon.
- **Hiding service decisions** (2026-11-21): who is served is TisVlay's desktop-session rule, asked of logind once per connection through its login library; a user switch leaves pads hidden; the socket and records live under `/run/ctrlassist`; the step landed in a few commits with one stop.
- **Naming:** the root helper is called the hiding service in the architecture and `ctrlassist-hider` as a crate, because "helper" also described the assisting player. Older sections of this plan still say helper.
- **Branch:** `feat/rewrite`, one linear campaign branch. Nothing is pushed.
- **Template source:** the maintainer's TisVlay checkout. Record the TisVlay commit copied from in each commit that copies from it.
- **Hardware on the development host:** Xbox and DualShock/DualSense pads, Steam. No wheel or HOTAS.
- **Legacy baseline:** 0.4.0 built from a `git worktree` of `legacy/0.4` at `../legacy-0.4`, which carries its own flake.
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

**Privilege is confined to a minimal helper.** The daemon never runs as root. The hiding spike found no unprivileged way to make a pad absent to games ([architecture](../architecture.md#hiding)), so the native packages install a small root helper that does one thing: on request it installs a runtime udev rule denying a pad's event, js and hidraw nodes to everyone, opens them itself and passes the open descriptors to the daemon, and removes the rule and restores the nodes when the daemon's connection closes, crash included. It hands over descriptors rather than permissions because the daemon and the games are the same user. Hiding is the rule followed by re-binding the pad's driver, which cuts off programs that already had the pad open, as Steam does from plug-in; restoring re-binds again, so they see the pad return. The daemon names a pad to the helper by passing an open descriptor it already holds, never a path or a name: possession proves it could already read that pad, and no attacker-chosen string reaches a rule. The helper refuses anything that isn't a joystick the seat user could open, builds rule text only from parsed numbers, and opens hidraw only for an allowlisted driver. The active seat user may connect to it, enforced on its socket the way TisVlay enforces its own; no prompt. Channels that can't install it, the Flatpak above all, fall back to a grab plus Steam's blacklist, with the gaps the finding lists.

**InputPlumber is out of scope for the first release.** CtrlAssist works independently and documents that the two shouldn't manage the same pad. Cooperation could not be tested on the development host and would be a second, unexercised code path.

**Daemon and thin clients, as separate processes of one binary.** `ctrlassist service daemon` owns the pads and the graph. The CLI, tray, GUI and TUI are clients. Reasons to split rather than run one process: a GUI toolkit shouldn't share a process with the input loop; a client crash shouldn't drop the virtual pad mid-game; a gamescope session may have no tray; and the engine must outlive a settings window.

**Flatpak is a second-tier channel.** It can run the same user-level processes inside its sandbox and gets whatever hiding works there. It matters because SteamOS and Bazzite are immutable, but it is not a first-release gate.

**The core abstraction is a routing graph** of sources, nodes and sinks, with feedback as part of the model from the first test. It unifies mux, demux, the legacy cookbook's chained instances and splitting one pad into several (#53), and it is what a profile describes (#6). Profiles are validated as acyclic.

**Crates.** `ctrlassist` on crates.io becomes the client SDK; the binary lives in `ctrlassist-cli`, as in TisVlay. `cargo install` is no longer a supported install path, because a daemon needs a unit file and udev rules cargo can't install.

**zenoh for the control plane,** as in TisVlay, over a Unix socket in `$XDG_RUNTIME_DIR`. The alternative is the session D-Bus through zbus, which is already in the tree for the tray and would give activation and name ownership for free. zenoh is chosen because TisVlay served D-Bus first and moved off it for type safety and API ergonomics, and because its protocol, SDK and state-watching patterns carry over directly. Performance work belongs to the data path, which never touches it (ground rule 5). S3 checks only that sharing a process with it is harmless.

**Presets first, graphs in profiles.** Clients offer presets; arbitrary graphs can be written in a profile file from the first release; no client edits them visually.

**Disconnects hold and re-bind.** When a pad in a setup drops, the virtual pad stays alive so the game never sees a disconnect, the missing pad's inputs go neutral, the other pad keeps working, and the same pad is re-bound and re-hidden when it returns. A notification says what happened.

**Virtual pads come in two kinds, chosen by identity** ([architecture](../architecture.md#virtual-pads)). A uinput pad serves the generic identity and Xbox identities. A PlayStation identity needs a uhid pad, because SDL and Steam ignore an evdev node carrying Sony's IDs. The default, when a profile names none, is the generic one: a virtual pad that claims to be real hardware is a choice. Copying a pad means its IDs on a uinput pad for Xbox-family pads and a uhid copy for PlayStation pads.

**The DualSense identity and the Steam blacklist fallback are in the first release, built last.** Everything else is brought up end to end with generic and Xbox identities and the helper first, so either can slip to a point release without holding 0.5.0. Two reviewers recommended deferring both; the maintainer chose to keep them.

**A DualSense identity is copied from a connected pad, never shipped.** Its report descriptor and feature reports are read from a real DualSense and cached per pad in the user's state directory, so the identity is offered only to someone who has had one connected, and nothing of Sony's is carried in the repository.

**Feedback routes rumble everywhere and passes the rest through between like pads.** Rumble is translated between the two forms (force-feedback requests on uinput, output reports on uhid). When the sink and the source are the same kind of pad, the whole output report passes through, so LEDs and trigger effects work; between unlike pads they are dropped.

**Pads are read directly through evdev, on one epoll thread** ([architecture](../architecture.md#the-input-path)). gilrs leaves the dependency set: it opens pads by path, so it can't take the descriptors the helper hands over, and its loop wakes about a hundred times a second when idle where the new one never does. The first release includes its own parser for SDL's controller database, so pads outside the Xbox and PlayStation families, which gilrs used to normalize, keep working.

**The input thread asks for real-time scheduling and degrades quietly.** If the kernel refuses, the daemon runs normally and its status says which. Packages raise the limit where they can; rtkit is not used.

**Activity is split between the source and the node.** A source has a deadzone that zeroes its rest noise before any node sees it, since a worn stick is a fact about the pad. A mux node has an engage threshold and a lower release threshold on what remains, since "is the assist touching this?" is the mode's question and two thresholds stop a hovering stick from flickering between owners. Defaults match the legacy 0.1, so most profiles set neither. No ownership is held by a timer.

**Ownership is over groups, not pairs.** A group is a set of controls owned together: a stick's two axes, a yoke's three, a flight grip's axes with its trigger and hat, a lone button or throttle as a group of one. One rule replaces legacy's separate cases: per group, whoever is active on it owns every member. A gamepad's groups are derived from the well-known control names, so an ordinary profile declares none; a profile may declare or regroup them for other devices.

**A mode is a merge rule per group.** Legacy priority mode already had an exception (triggers aren't owned; the larger value wins), so the routing graph spike models each mux mode as a table of rules over the same machinery (owned, largest, blended, switched) and reports whether that expresses all three without special cases.

**On a switch, the virtual pad takes the incoming pad's state.** What both pads hold stays down with no blip, what only the outgoing pad held goes up, and analog values step to the incoming value. After any switch the sink equals the active source, which is an invariant the tests check.

**A game's rumble upload is answered leniently.** It is told the effect is accepted if any target pad took it, and also when rumble is switched off; it is refused only when every pad asked refused it. A game that insists on rumble then still runs when one pad lacks it.

**A profile names a pad by address or by model.** By address when it must be that exact pad, which is what a client records when a setup is created; by model ("any DualSense") when any will do, which is what makes a profile shareable between machines.

**Pad state holds 64 buttons and 16 axes.** Measured against 128 and 32, which costs about three times as much per step. Raising it later means first changing the step to visit only the positions in use.

**A daemon restart drops its virtual pads, for now.** The game sees a disconnect and the helper restores the hidden pads, so it falls back to the real ones. Keeping virtual pads alive across a restart is an idea, not first-release work.

**Rumble is routed as levels, mirroring the kernel.** The router keeps each game's effects and their timing, computes one strong and one weak level per pad, and keeps a single effect on each pad updated to it, as the kernel's own layer for rumble-only pads does. Within one virtual pad, playing effects add and clamp; when two virtual pads rumble one pad at once, the strongest request wins on each motor. The alternatives and the prior art are kept in [ideas/rumble-routing-model.md](../ideas/rumble-routing-model.md).

**Hiding a pad disconnects it from whatever had it open,** when a setup starts and again when it stops. That is accepted: the real pad vanishes from running games and Steam and the virtual one appears, with nothing restarted.

**The switch is an input to the node, not a pad event it happens to see.** In the first release it can be raised by one button named in the profile (default Mode, consumed rather than forwarded), on whichever source the profile names, or by a client through a daemon rpc. Modeling it as a signal means later triggers don't change the node.

**The legacy words stay, for users too:** primary and assist for the roles; mux with priority, average and toggle; demux with unicast and multicast. Two alternative families were tried and read worse: these are precise, already paired, already in the README and issues, and "assist" is the project's name. Clients add a one-line description beside each, since the mode names are jargon to a newcomer.

**Setups start manually, with optional auto-start.** A client starts a setup. A profile may also be marked to start by itself once the daemon is up and all of its pads are connected.

**Clients are built in the order CLI, tray, GUI, TUI.** The GUI is Slint, following TisVlay. The TUI uses ratatui and comes last because TisVlay has none to pattern it on; the `archive/feature/tui` tag holds an earlier draft worth reading first. Each can mirror live pad state, which the daemon publishes on change and only while something subscribes.

**The first release is daemon, CLI and tray,** matching the surfaces 0.4.0 has (a CLI and a tray). The GUI and TUI follow as later minor releases. It is verified on NixOS with KDE, the maintainer's host; the deb, rpm and Arch packages are built and container-tested but not hardware-verified.

**The event model isn't gamepad-shaped, but only gamepads are built.** See [Event model scope](#event-model-scope).

## Provisional, pending spikes

**Pads are identified by a stable hardware ID,** falling back to vendor and product plus port, then name. The hiding spike found the kernel's `uniq` value to be the pad's own address on both pads tested, and Steam keys its per-pad configuration by it; whether it is the same over USB and Bluetooth, and what an Xbox pad reports, is the one check still open.

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
- **Feedback kinds are open** (a force-feedback effect, a device's output report), so a wheel's constant-force and spring effects pass through even though only rumble is tested.
- **Motion and touch kinds are reserved.** A uhid sink could carry them; nothing in the first release produces them.

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

The legacy dependencies were chosen a year ago, and the Rust input ecosystem moves quickly. Before the spikes pick their building blocks, survey it and write a finding:

- **Legacy dependencies** (`gilrs`, `evdev`, `udev`, `ksni`, `notify-rust`, `ashpd`, `fs2`, `dirs`, `signal-hook`, `parking_lot`, `env_logger`, and the common ones TisVlay already pins): the current release, what changed since the pinned version (gilrs's changelog especially), maintenance state, and what TisVlay uses in that role.
- **Alternatives by goal:** evdev and uinput access; hidraw and uhid (the `archive/uhid` tag tried `uhid-virt` and `hidapi`); controller mapping databases; HID report descriptors and ready-made controller emulation, PlayStation above all; hotplug (udev bindings against pure-Rust netlink); epoll-style loops beside tokio; ratatui; graph or dataflow crates worth borrowing from.
- **Prior art:** how InputPlumber, Steam Input and SDL handle hiding, identity and routing, under which licenses, and how handhelds with built-in controllers present them.

Each entry records its source and date. The survey chooses nothing: it gives each spike its candidates. Done when every bullet above has an entry and each spike below has a candidate list.

## Spikes

Each spike is a workspace under `spikes/<name>/`, run on the development host, with the maintainer asked only for physical actions such as pressing buttons, batched into one checklist per session with results captured to a log. On that host the maintainer has allowed: grabbing and releasing pads; editing Steam's config, backed up first and restored after each test; restarting Steam; and `sudo` for temporary udev rules and node permissions, each command shown before it runs and undone afterward. Each ended in a finding holding a results table and a verdict, with a stop for review. The findings were then folded into `docs/architecture.md` (hardware caveats into `docs/concerns/`) and deleted; git history has them. Spike code is deleted in the commit that lands its replacement in `crates/`, except S4, whose tests and nodes are promoted to become the core crate.

Order: the survey, then S1 and S5, then S2, then S4, then S3. S5 precedes S4 because the sink type decides what feedback looks like; S1 precedes S2 because a grab decides who may read the pad.

### S1. Hiding, permissions and identification

*Question:* can an unprivileged daemon make a pad absent to games, and what must the packages install?

- [x] For each mechanism (evdev grab; Steam's blacklist; SDL's ignore-devices hint per launch; node permissions and a runtime udev rule set by root) and each consumer (Steam, read from its controller log; an SDL test tool run natively on both of its backends; raw evdev and joydev readers), record: absent, present but silent, or visible. A Proton game is left to a later check.
- [x] For each mechanism, record what remains after the daemon is killed without cleanup, and after the pad reconnects.
- [x] Record Steam's behavior by start order and whether a change needs a Steam restart.
- [x] In a clean NixOS VM with no Steam or Sunshine rules, defined as a flake check, with fake pads created through uinput and uhid: what a seat user can open among `/dev/uinput`, `/dev/uhid` and the pads' event, js and hidraw nodes.
- [x] The root mechanism against SDL on a real pad.
- [ ] Record which identifiers stay stable for an Xbox pad and a DualShock or DualSense across reconnects, over USB and Bluetooth (recorded for the two pads connected; the reconnect and transport comparison needs the maintainer).
- [x] Write the finding.

*Verdict:* a mechanism qualifies as the default only if it reaches "absent" for Steam Input and for a native SDL game, and leaves nothing behind after a kill. If no unprivileged mechanism qualifies, the helper ships, and the finding specifies the smallest interface it needs and what it costs to package.

*Feeds:* the provisional helper decision, pad identification, both hide rows, the udev rules.

### S5. Virtual pad identity

*Question:* what does it take for a virtual pad to pass as a specific pad, PlayStation above all (#5)?

*Background:* SDL reads DualShock and DualSense pads through hidraw by default, and Steam is believed to as well. A uinput pad carrying Sony's IDs has no HID device behind it; a uhid pad does, and the kernel's PlayStation driver can bind to it. InputPlumber emulates a DualSense this way.

- [x] Build a uinput virtual pad with a DualSense's copied IDs, and a uhid one replaying a real DualSense's report descriptor.
- [x] For each, record what Steam and an SDL test tool report: detected as what, and rumble. Layout under real input, glyphs, gyro, touchpad and LEDs were not exercised and are listed in the finding.
- [x] Repeat for an Xbox identity (virtual only; no Xbox pad was connected).
- [x] Record what feedback a uhid sink delivers (output reports) and what it needs installed (`/dev/uhid` access).
- [x] Write the finding.

*Verdict:* uinput only, uhid for named identities, or uhid throughout, and which identity is the default when a profile names none. uhid is kept for an identity only if it is detected as that pad with correct glyphs and working rumble where uinput is not.

*Feeds:* identity, feedback's representation, the reserved motion, touch and LED kinds, the udev rules.

### S2. The input loop

*Question:* what does the data path look like, and what reads the pads?

- [x] Build fake pads through uinput so the loop runs without hardware, and a harness that timestamps an event entering a fake pad and leaving a virtual pad.
- [x] Build the loop: one thread blocked in epoll over the pads and the virtual pad's feedback requests, processing whole report frames, resyncing on dropped events, answering feedback uploads.
- [x] Decide the reader: direct evdev, settled by gilrs opening pads by path and keeping its mapping parser private. A pad that needs a mapping was not on hand to test.
- [x] Measure the loop and the legacy 0.4.0 binary, on an idle host and with every core loaded: latency median and tail, CPU at a sustained high event rate from two pads, and wakeups while idle.
- [x] Record whether the input thread needs real-time scheduling to hold its tail under load, and what granting that costs to package.
- [ ] Exercise suspend and resume and a Bluetooth drop, and record the race between a returning pad being re-hidden and a game seeing it (needs the maintainer at the machine).
- [x] Write the finding.

*Verdict:* the loop is kept if it makes no wakeups while idle and is no worse than legacy on latency tail and CPU in every measured condition. The reader is the cheapest option that merges two different pads correctly.

*Feeds:* the device crate, the daemon's threading, #10.

### S4. The routing graph

*Question:* what is the node API, and does one model express every row of the feature table?

- [x] In a pure crate with no device or async dependencies, write scenario tests first: each mux and demux mode; a chained demux into mux; feedback for each (upload, play, stop, erase, with replies); rumble routed to a pad other than the one providing input; a target rejecting an upload; two sinks driving one pad; a pad dropping out and returning; and one source that isn't gamepad-shaped. Gain was left out.
- [x] Write the nodes and an executor that make them pass, with nothing wired around the executor.
- [x] Add property tests for the invariants.
- [x] Benchmark one executor step and compare it with the input loop's per-event cost.
- [x] Draft the profile format for three presets and one hand-written graph.
- [x] Write the finding.

*Verdict:* the model is kept if every scenario passes through the executor without a special case, a step allocates nothing, and its cost is below S2's measurement noise. The open-ended vocabulary is kept if the non-gamepad scenario needed no change to node code.

*Feeds:* the core crate (by promotion), the profile format, the event model scope.

### S3. zenoh beside the input loop

A check, not a comparison.

- [x] Put the input loop in a daemon that also runs a zenoh session on a Unix socket, publishing state and serving an rpc in TisVlay's style, with one idle client attached and the input thread kept off zenoh's runtime.
- [x] Re-run the loop's measurements; record the daemon's idle wakeups and resident memory, and a one-shot client call's time from start to reply.
- [x] Prototype the pad-state mirror: published on change, only while subscribed.
- [x] Write the finding.

*Verdict:* zenoh stays if the input thread's latency and wakeup numbers fall within the run-to-run spread S2 recorded. If not, the finding says what isolates them before any other IPC is considered.

*Feeds:* the daemon's threading model.

## Steps

1. **Clean slate and minimal scaffold.**
   - `chore: remove the legacy tree`, after creating `legacy/0.4`; keeps `LICENSE`, the artwork and this plan.
   - `build: toolchain, dev shell and file checks from the TisVlay template`: exact-pinned `rust-toolchain.toml`, the flake dev shell, a `justfile` and `prek.toml` with the file checks and `typos` (allowing the legacy branch name), `docs/plans/README.md` listing this plan, and a `CLAUDE.md` carrying the working rules and terms above. No cargo gates yet: a workspace with no members fails them.
   - Verify: `prek run -a` green; `nix develop -c just` lists the recipes.
2. **Ecosystem survey.** Verify: its done-condition above.
3. **Spikes,** in the order above, each with its checklist. Verify: each finding's results table and verdict.
4. **Architecture.** Done: [docs/architecture.md](../architecture.md), reviewed with the three lenses and revised; concerns and ideas written; the legacy issues triaged into ideas or closed by the design (#6 by profiles, #8 by dropping gilrs, #9 by packaging, #10 by the input thread).
5. **Build.** Commit-sized steps, in slices that can be cut into PRs later. Each step brings its tests and passes the gates that exist when it lands; tooling arrives with the step that first uses it. "VM" is the NixOS virtual machine flake check; "hardware" is a named check at the maintainer's machine; **stop** means wait for the maintainer before going on.

   **Slice A: workspace and core**

   1. `docs: delete the spike findings` — their content is in the architecture, concerns and ideas. Verify: no link into `plans/findings` remains.
   2. `feat(core): promote the routing graph to ctrlassist-core` — the root workspace (excluding `spikes/`), the workspace lint table and profiles trimmed from TisVlay, the spike moved with `git mv` and renamed, tests as one integration binary, the allocation count as a test, the wide-capacity feature dropped. Brings the `fmt`, `build`, `clippy`, `doc` and `test` recipes and hooks, the nextest config, `docs/build.md` and `docs/conventions.md`, and the crate layout in CLAUDE.md. Deletes `spikes/graph`. Verify: `prek run -a`; 36 tests. **Stop:** review the lint table.
   3. `fix(core): validate everything a profile can say` — control indices, source and input counts, thresholds, non-numbers; typed errors; ungrouped controls as groups of one; deadzones on mixed groups; selection skipping absent inputs; hat and trigger-button rules stated for the sink. One test per defect listed in the architecture review, and a property test that no valid graph panics.
   4. `feat(core): level-based rumble routing` — replaces the per-effect router: effects with duration, delay, repeat and gain; one level per pad; add-and-clamp within a sink, strongest across sinks; the next-change time for the caller's timer. The scenario and property tests are rewritten around levels, with a model pad that behaves as a real one does on a refused update. **Stop:** review the core API before anything depends on it.
   5. `ci: checks, tests and coverage` — `ci.yml`, the instrumented test recipe and coverage ratchet, MSRV, `actionlint`, `zizmor`, dependabot. Verify: the recipes locally; the workflows first run after the repository move.

   **Slice B: device and hiding**

   6. `feat(hider): the hiding service and its client` — wire types in the crate's library; the descriptor-named `hide`, `reopen` and `restore`; the pad checks; rule text from parsed numbers; the re-bind for each driver kind observed; the seat check; records under `/run` and reconciliation; socket and service units. Brings `ctrlassist-test` with fake pads, the VM check, `docs/testing.md`, and the `deny` and `shear` gates. Deletes `spikes/hiding`. Verify: VM (hide, kill the client, restore, reconnect, a non-seat user refused, a keyboard refused, a hostile pad name harmless). Hardware: a DualSense against SDL and a running Steam. **Stop:** read the root code.
   7. `feat(device): read a pad from a descriptor` — frames, resync after dropped events, kernel-driver layouts, identifiers. Verify: VM with fake pads; tests skip visibly where uinput isn't writable.
   8. `feat(device): uinput virtual pads` — generic and Xbox identities, the own-output marker, force-feedback requests feeding the router, the packaged uinput and uhid udev rule. Verify: fake pad to virtual pad read back; a seat user in the VM. Hardware: SDL and Steam list both identities.
   9. `feat(device): the input thread` — epoll, the command queue and wake descriptor, the rumble timer, the real-time request, panic isolation per setup, and the latency bench as a binary. Deletes `spikes/loop`. Verify: a mux end to end in the VM; zero idle wakeups asserted; the bench against the numbers in the architecture. Hardware: rumble reaches a real pad and ends on time.
   10. `feat(device): hotplug, hold and re-bind` — Verify: a fake pad unplugged and replugged in the VM. Hardware: a Bluetooth drop, and a suspend and resume.
   11. `feat(device): controller database mappings` — the parser as untrusted-input code with a never-panics property test, and a fake pad with an odd layout. **Stop:** where the database comes from, how it is updated, and its license.

   **Slice C: wire, daemon, CLI**

   12. `feat(protocol): the profile schema and presets` — Verify: the architecture's example profiles as fixtures expanding to the expected graphs; validation errors that say where. **Stop:** the user-facing format.
   13. `feat: a daemon and CLI that report status` — session configuration, the rpc table, the version check, liveliness, the SDK's client and watcher, `service daemon`, `status`, logging, and the dev recipes. Verify: a wire test on a temporary socket.
   14. `feat(daemon,cli): pads, identities and identify` — Verify: wire tests with fake pads. Hardware: the buzz.
   15. `feat(daemon,cli): profiles` — the store with durable writes; validate, put, delete. Verify: wire tests.
   16. `feat(daemon,cli): setups` — start, stop, update, select; hiding; hold; auto-start; state. Deletes `spikes/zenoh`. Verify: VM end to end, including killing the daemon and finding the pads restored; the bench against the daemon. Hardware: priority, average and toggle in a native game and a Proton game; unplug; suspend. **Stop.**
   17. `feat(cli): shell completions`.

   **Slice D: tray**

   18. `feat(tray): the tray over the SDK` — Verify: menu tests from fixtures. Hardware: KDE.
   19. `feat(tray): notifications for hold, re-bind and missing rumble`.

   **Slice E: the two pieces built last**

   20. `feat(device): the DualSense identity` — the uhid pad, data copied and cached through the hiding service, a stable local address, neutral calibration, both rumble layouts, output-report pass-through with replay and reset. Deletes `spikes/identity`. Verify: the emulator against recorded reports. Hardware: a DualSense in SDL, Steam, a native game and a Proton game. May slip to a point release.
   21. `feat(device): Steam blacklist fallback` — with its record, reconciliation and removal. Verify: tests over real `config.vdf` shapes and a simulated crash. Hardware: Steam without the hiding service. May slip to a point release.

   **Slice F: packaging and release**

   22. `build(nix): package and NixOS module` — units, rules, the real-time limit; the VM check moves onto the module. Hardware: installed on the maintainer's host.
   23. `build: deb, rpm and Arch packages` — the dist profile, nfpm metadata, a container install test.
   24. `build(licensing): third-party notices`.
   25. `ci: release and audit workflows` — Verify: `actionlint` and `zizmor`; a real run needs the repository move.
   26. `docs(book): user guide with a generated CLI reference` — brings `ctrlassist-docgen`.
   27. `docs: contributing guide, security policy, issue forms`.
   28. `docs: retire the rewrite plan`.
   29. `chore(release): 0.5.0` — the maintainer tags, then runs the NixOS and KDE checklist.

   0.5.0 ships as packages only. The core, protocol and SDK crates are published to crates.io once a second client has exercised the API.
6. **Retire this plan** in build step 28, per the plans convention.

The repository move is the maintainer's, out of band, and gates none of these.

## Out of scope for the first release

Each becomes a `docs/ideas/` entry at step 4, with the trigger that would bring it in:

- The GUI and TUI (next minor releases).
- Wheel and HOTAS sources, and non-rumble force feedback (someone with the hardware to verify).
- Gyro, touchpad, LED and trigger-effect passthrough (S5 keeping uhid, then demand).
- A preset for splitting one pad (#53), and button debounce, which that issue also asks for.
- A visual graph editor.
- Button chords as a switch (a design for the first chord button, which otherwise leaks to the game or delays every chord member).
- Flatpak and Flathub (a maintainer or contributor on SteamOS or Bazzite, #13).
- Cooperating with InputPlumber where it is installed (someone on Bazzite who can test it).
- A bare binary tarball (a request from a distro none of the packages serve).
- Hardware verification of the deb, rpm and Arch packages.
- Keyboard and mouse as sources or sinks.

## Departures from the template's process

- **Build steps are deferred.** The plans convention asks for commit-sized steps with verification up front; here they are written at step 4, because the spikes decide the crates.
- **One plan for several spikes.** TisVlay gave its spike its own plan because it compared candidate technologies. These spikes answer different questions about one design, so they share this plan and get a finding each.
- **No issue or draft PR yet.** The contributing guide starts from an agreed issue and a draft PR. The maintainer is working locally until the repository moves, so the plan's history is this branch.
- **A user daemon,** where TisVlay's is a system service with polkit rules.

## For review

Decisions made while building through a marked stop, for the maintainer's QA. Each says what was chosen and what else was on the table.

None yet.

## Deviations

- **The hiding service passes no hidraw descriptor yet** (build step 6). The architecture has it open a hidraw node for an allowlisted driver. Nothing reads one until the DualSense identity (build step 20), and the path can't be exercised in the virtual machine without Sony's report descriptor, so it lands with that step. The rule denies hidraw nodes now.
- **The Steam-driving dislodge recipe went with `spikes/hiding`** (build step 6). It ran the spike's probe. Its replacements, `just hider-dev` and `just hide-pad`, go through the real service; starting Steam and reading what it lists is by hand.
- **`ctrlassist-hider` depends on `libc`** as well as `rustix` and `udev` (build step 6): the login library is C, and its strings are freed with `free`.
- **CI opens uinput on its runner** (build step 7), so the pad reader's tests run there and count toward coverage; without that the floor would fail in CI. Like the rest of the workflow, this first runs after the repository move, along with the system libraries the workflow now installs.
- **Own virtual pads are known by device number, not by a marker in `phys`** (build step 8). The `evdev` crate's call that sets `phys` is refused by the kernel (it declares the wrong request size), and the builder keeps its descriptor private, so the marker can't be set without replacing the crate's uinput setup. The daemon's virtual pads die with it, so remembering their device numbers does the same job. Worth reporting upstream.
- **The generic identity reports 0.4.0's numbers** (build step 8), USB 1234:5678, because those are what SDL and Steam were seen to adopt. They are the `evdev` crate's sample numbers, not ours; numbers of our own are a question for the release.
- **No CPU-time limit on the input thread** (build step 9). The architecture asked for one beside real-time scheduling. Exceeding `RLIMIT_RTTIME` kills the process, which would take every virtual pad with it; the kernel's own limit on real-time threads already protects the machine.
- **The queues are the standard library's bounded channels,** not a lock-free queue crate (build step 9): they are only ever polled from the input thread, never block it, and add no dependency.
- **The bench no longer times 0.4.0** (build step 9). It runs the input thread in its own process; the comparison is in the architecture. `spikes/zenoh` stays until its step, but its bench recipe went with `spikes/loop`, whose binary it drove.
- **Panic isolation is untested** (build step 9): nothing in a setup's handling can be made to panic from outside, which is the point, so the guard is exercised only by reading it.

The drafting history is in git.
