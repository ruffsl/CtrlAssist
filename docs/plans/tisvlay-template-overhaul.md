# Overhaul CtrlAssist on the TisVlay template

Status: **draft, awaiting maintainer review.** Nothing below is implemented.

## Goal

CtrlAssist was a first Rust and Linux-desktop project: one crate, 4,000 lines, no tests, a two-job CI, and a tray that owns the mux threads directly. [TisVlay](https://github.com/TisVlay/TisVlay) grew out of the same lessons and is now the more mature codebase. This plan rebuilds CtrlAssist on TisVlay's project template and architecture, and moves the repository to the `CtrlAssist` GitHub org.

Why it's worth doing:

- **The open issues are mostly architectural.** Hotplug handling (#8), prebuilt packages (#9), a GUI (#7), richer configuration (#6), latency work (#10) and per-control splitting (#53) all want things the current shape doesn't have: a long-lived owner of the devices, a testable core, a release pipeline.
- **Nothing is tested.** The mode logic reads live `gilrs::Gamepad` state, so it can't run without hardware. Every change is verified by hand with two controllers.
- **One maintainer, two projects.** The same `just` recipes, lint policy, CI shape, docs layout and release steps in both repos means one set of habits.

Out of scope: new mux or demux modes, new controller features (gyro, adaptive triggers), keyboard and mouse input. Existing issues that this plan doesn't close move to `docs/ideas/`.

## How this plan is organized

The work is six phases, each one PR (phase 0 is mostly repo administration). `main` stays releasable after every phase.

This is a deliberate departure from the one-plan-one-PR convention in TisVlay's `docs/plans/README.md`: this file is the roadmap and stays in `docs/plans/` across all phases. Each phase PR ticks its phase here and records deviations in the same commit that departs from the plan. Phases 1 to 3 have commit-sized steps below. Phases 4 and 5 state their decisions and a step outline; each gets its own `docs/plans/<phase>.md` with full steps when it starts, because their details depend on what phases 2 and 3 turn up. The last phase retires this file.

| Phase | PR | Result | Release |
|---|---|---|---|
| 0 | Repo move | `CtrlAssist/CtrlAssist`, new app ID | none |
| 1 | Scaffolding | TisVlay's tooling, CI and docs layout around the unchanged crate | none |
| 2 | Workspace, conventions, tests | Same behavior, split into crates, lint-clean, tested | none |
| 3 | Packaging and release | deb, rpm, tarball, Nix package and module, book | 0.5.0 |
| 4 | Daemon, protocol, clients | A user service owns the devices; CLI and tray are clients | 0.6.0 |
| 5 | GUI | Slint settings window, i18n | 0.7.0 |

## What exists today

Findings from reading the code, which the decisions below rest on. Reviewers: check these.

- **Single binary crate** `ctrlassist` 0.4.0, published on crates.io, edition 2024, `rust-version = "1.91.1"`, toolchain floating on `stable`. Verbs: `list`, `mux`, `demux`, `tray`.
- **Threads, not tasks.** `mux::manager::start_mux` spawns an input thread (`gilrs.next_event_blocking`) and a force-feedback thread (blocking `VirtualDevice::fetch_events`). Shutdown is an `AtomicBool` plus a no-op FF event written to the virtual device to unblock the FF thread.
- **Live settings** go through `RuntimeSettings` (`Arc<RwLock<…>>` plus an `ff_generation` counter) that the tray writes and the threads poll.
- **Mode logic is coupled to gilrs.** `MuxMode::handle_event` takes `&Gilrs` and reads the other pad's current state from it, and returns `evdev::InputEvent`s.
- **The tray owns everything.** `tray/app.rs` (742 lines) holds the `Gilrs` instance, the selected controllers, the running operation's thread handle, config persistence and notifications. The CLI verbs duplicate the start logic in `main.rs`.
- **Hiding** is RAII (`ScopedDeviceHider`): `system` chmods device nodes to 0600 and needs root; `steam` edits `controller_blacklist` in Steam's `config.vdf` under a file lock.
- **Errors** are `Box<dyn Error>` built from strings. **Logging** is `log` + `env_logger`, with most messages also sent through `println!`.
- **No tests, no `unsafe`.**
- **Likely bugs** (to confirm with a failing test in phase 2, per the contributing guide):
  - `mux::runtime::run_input_loop` and the demux equivalent check for a mode change only in the outer loop, which is reached only after `next_event_blocking` times out (one second with no input). A mode changed from the tray while a stick is held doesn't apply until input pauses.
  - The FF loop checks `ff_generation` only between `fetch_events` calls, which block. A rumble-target change applies on the next FF event from the game, not at once.
  - `GamepadResource::clone` reopens the device and panics if that fails (a pad unplugged at the wrong moment). `wait_for_virtual_device` unwraps `dev.name()`.
  - `run_mux` / `run_demux` `expect` inside the worker thread, so a start failure is a panic message rather than an error exit.
  - `TrayConfig::config_path` creates the config directory as a side effect of reading.
- **Packaging:** a Flatpak bundle built on release (`io.github.ruffsl.ctrlassist`), `cargo install`, a dev-shell-only `flake.nix`. Artwork is in Git LFS.
- **Uncommitted work on `main`:** a staged `.dockerignore` and a modified `Dockerfile` that build the Flatpak in Docker. This plan doesn't touch them; phase 3 decides their home (see D12).

## Decisions

### Repository

**D1. Transfer the repo, don't recreate it.** `ruffsl/CtrlAssist` → `CtrlAssist/CtrlAssist` with GitHub's transfer. Stars, issues, PRs, releases, discussions and LFS objects come along, and old web and git URLs redirect as long as no new `ruffsl/CtrlAssist` is created. A fresh repo would lose all of that for no gain.

**D2. Move first.** Everything phase 1 and 3 set up is keyed to the repo's owner: required checks, environments, Codecov, Read the Docs, crates.io trusted publishing, attestation signer identity. Doing it once against the final location avoids redoing it.

**D3. App ID becomes `io.github.ctrlassist.CtrlAssist`.** It matches the org, needs no domain, and is the form Flathub verifies for a GitHub org. Cost: an existing Flatpak install is a different app, so users reinstall, and settings under `~/.var/app/io.github.ruffsl.ctrlassist/` aren't carried over. The release notes say so. The ID is used for the desktop file, metainfo and icon whether or not the Flatpak continues (D12).

### Template adoption

**D4. Adopt TisVlay's tooling as is, minus what CtrlAssist has no use for.** Taken: `justfile` as the single source of commands, `prek.toml` dispatching to it, `.config/` (clippy, nextest, typos, tombi, actionlint, release-plz), the exact-pinned `rust-toolchain.toml`, the flake dev shell providing every tool, the workspace lint table, the `dev`/`test`/`release`/`dist` profiles, SHA-pinned workflows, dependabot with cooldown, issue forms, `CONTRIBUTING.md`, `SECURITY.md`, `.claude/CLAUDE.md` importing `docs/conventions.md`, and the `docs/` layout (`architecture.md`, `build.md`, `testing.md`, `conventions.md`, `plans/`, `ideas/`, `concerns/`, `bugs/`, `book/`). Left behind: CEC, SSDP, polkit, greeter, Cachix, the Slint pieces until phase 5.

Reason for copying rather than re-deriving: the files are the maintainer's own, same license, and each carries reasons in its comments that took TisVlay 270 PRs to learn. Each copied file is edited for names and trimmed, not rewritten.

**D5. Tooling lands before code changes.** Phase 1 keeps the single crate and today's `clippy -D warnings` level. The strict lint table arrives in phase 2 one lint at a time, each commit fixing that lint's sites, following TisVlay's rule that a lint lands at zero sites. Mixing "new CI" and "every file touched" in one PR would make both unreviewable.

**D6. MSRV and toolchain follow TisVlay's values at the time of phase 1.** Same reasoning as there: the pin makes a build reproduce from a SHA, and the MSRV must not exceed nixos-unstable's rustc because the Nix package builds with the consumer's compiler.

### Code structure (phase 2)

**D7. Workspace layout.** One shared version, `crates/` members:

| Crate | Holds | Errors | Published |
|---|---|---|---|
| `ctrlassist-core` | Vocabulary (`MuxMode`, `DemuxMode`, `Hide`, `Spoof`, rumble targets), the pure mode logic, FF effect bookkeeping (`EffectManager`), the Steam VDF blacklist edit as a string transform | `thiserror` | decided with D13 |
| `ctrlassist-device` | Everything that touches the kernel: gilrs discovery, evdev/uinput virtual pads, FF forwarding, the two hiders, the input and FF loops | `thiserror` | no |
| `ctrlassist-runtime` | Process glue copied from `tisvlay-runtime`: `logging::init` (tracing + journald), atomic config writes | `anyhow` | no |
| `ctrlassist-tray` | ksni tray, notifications | `anyhow` | no |
| `ctrlassist` | The binary and its clap tree as a lib (for docgen) | `anyhow` | decided with D13 |
| `ctrlassist-test` | uinput-backed fake gamepads for integration tests | — | no |
| `ctrlassist-docgen` | CLI reference for the book (phase 3) | `anyhow` | no |

Departure from TisVlay: there the bare name `tisvlay` is the client SDK and `tisvlay-cli` holds the binary. Here `ctrlassist` on crates.io is already the binary that `cargo install ctrlassist` installs, so the binary crate keeps the name and the phase 4 SDK is `ctrlassist-client`.

**D8. The mode logic stops depending on gilrs and evdev.** Modes take an event plus a `PadState` snapshot of both pads (a small trait or struct in core, implemented for `gilrs::Gamepad` in the device crate) and return core-defined output events that the device crate converts to `evdev::InputEvent`. That is the seam that makes the modes unit- and property-testable (for example: priority mode never emits a primary event while assist holds the same button; average mode's output stays in range; toggle emits a full resync on switch).

**D9. Real-kernel integration tests through uinput, not mocks of evdev.** `ctrlassist-test` creates "physical" pads with uinput, the test runs a real mux or demux against them and reads the virtual pad's output. These need `/dev/uinput`, so they skip when it isn't writable and run for real in the NixOS VM check (phase 3). This is TisVlay's "hermetic tests via seams" idea applied where the seam is the kernel's own injection interface. To verify on first use: that gilrs picks up uinput pads inside the VM (it enumerates through udev).

**D10. Conventions are TisVlay's `docs/conventions.md`, trimmed.** `tracing` with structured fields; no `println!` (CLI output through `writeln!`, `BrokenPipe` mapped to success); `expect("… because …")` over `unwrap`; reasoned `#[expect]`; `#[must_use]` on handles whose drop stops work (`MuxHandle`, `ScopedDeviceHider`). The wire and secrets bullets are dropped until phase 4 gives them something to govern.

### Packaging (phase 3)

**D11. Ship deb, rpm, a binary tarball, and a Nix package with a NixOS module.** Confirmed by the maintainer. The pipeline is TisVlay's: `cargo auditable zigbuild --profile dist` against glibc 2.34 for x86_64 and aarch64, `cargo nfpm` for deb and rpm, Sigstore build provenance and SBOM attestations, a draft GitHub release with checksums. The tarball is new relative to TisVlay: the same binary plus completions, license files and the udev rule, for distros with neither format. This closes #9.

The packages ship a udev rule for `/dev/uinput` access and, from phase 4, a systemd user unit. To verify on hardware first: what grants uinput access on current installs (the README never mentions it, so something on the tested distros already does, probably Steam's udev rules), so the rule neither duplicates nor fights it.

**D12. Flatpak: undecided; recommendation is keep it.** TisVlay rejected Flatpak, but its audience isn't CtrlAssist's. SteamOS and Bazzite are immutable, a deb or rpm doesn't install cleanly there, and they are where a couch-gaming accessibility tool is most wanted (#13). If kept: the manifest moves to `packaging/flatpak/` under the new app ID, CI builds it on the release tag, and the staged Dockerfile becomes `packaging/docker/` or is dropped in favor of the CI job. Keeping it constrains phase 4 (D16). **Needs a maintainer decision before phase 3 starts.**

**D13. crates.io: undecided; recommendation is keep publishing.** After the split, `cargo install ctrlassist` works only if every internal crate it depends on is published too, each then carrying TisVlay's public-API policy (`#[non_exhaustive]`, semver checks, a README). That is real upkeep for an install path the tarball and packages mostly replace. Against dropping it: 0.4.0 is there now, the README badges point at it, and it is the only route for distros without a package. If kept, publishing moves to trusted publishing through release-plz as in TisVlay, and cargo-binstall metadata points at the tarball. If dropped, a final 0.4.x points installers at the releases page. **Needs a maintainer decision before phase 3 starts.**

**D14. User docs move to an mdBook on Read the Docs.** The README's modes, usage, hiding, configuration, FAQ and cookbook sections become book pages; the README keeps the banner, pitch, features, a screenshot and links, with `ANCHOR: overview` markers the book includes. The CLI reference is generated by `ctrlassist-docgen` from the clap tree and drift-gated in CI.

### Architecture (phase 4)

**D15. A per-user daemon owns the devices; CLI, tray and GUI are clients.** The daemon holds the one `Gilrs` instance and a set of *pipelines* (a mux or a demux, each with its virtual devices, hider and FF routing). Clients create, alter and stop pipelines and watch state.

What this buys, tied to issues:

- **Hotplug (#8):** one place sees connect and disconnect, marks a pipeline degraded, and re-binds when the pad returns, instead of a panic in `clone` or a dead FF thread.
- **Multi-pipeline setups without `sleep 1`:** the cookbook's chained mux and demux scripts become ordered requests to one daemon that knows when each virtual device exists. Multiple tray icons for multiple instances go away.
- **One start path:** the CLI and tray stop duplicating setup logic.
- **A GUI has something to talk to (#7).**

It is a *user* service, not a system one as TisVlay's daemon is: the devices belong to the seat user, Steam's config is in their home, and there is no multi-user or greeter concern.

**D16. The daemon is a library that also runs in-process.** `ctrlassist service daemon` runs it under systemd. `ctrlassist mux …` and `ctrlassist demux …` stay as foreground verbs with today's flags: with a daemon running they create a pipeline tied to the command's lifetime; with none they host the daemon stack in-process for the one pipeline. Reasons: existing scripts and the cookbook keep working, `sudo ctrlassist mux --hide system` keeps working, and a Flatpak (D12), which can't install a systemd unit, can run tray plus daemon in one sandboxed process.

Consequence to accept: `--hide system` isn't available from the unprivileged user service. The tray offers it only when the daemon reports it can do it. Doing it properly (a udev-rule or ACL mechanism, #19) is an idea doc, not this plan.

**D17. IPC is zenoh over a Unix socket in `$XDG_RUNTIME_DIR`, reusing TisVlay's protocol patterns.** Cached publishers for state, queryables for rpcs, a liveliness token for presence, an `endpoint` table declaring each rpc once, a typed `WireError`, a `major.minor` release check between client and daemon. New crates: `ctrlassist-protocol`, `ctrlassist-daemon`, `ctrlassist-client`.

Reason: the maintainer has already paid for this stack's lessons (TisVlay's `architecture.md` § Empirical zenoh laws), and the protocol, SDK and `StateWatcher` code carries over nearly unchanged. The topology is simpler than TisVlay's: one socket, the daemon as router, clients in client mode, no system/user split.

Alternatives considered: serving D-Bus with zbus (already in the tree through ksni and notify-rust, and friendlier to Flatpak, but TisVlay shipped that and removed it); a hand-rolled JSON-lines socket (smallest, but reinvents state replay and presence). Zenoh's cost is dependency weight and build time for what is a local control plane. **This is the decision most worth challenging in review.**

**D18. No `bevy_ecs`.** TisVlay uses a world for many interacting per-TV state machines. CtrlAssist's daemon state is a device list and a handful of pipelines; a single reactor task owning that state, with commands in and events out, is enough. Revisit if per-device state machines multiply.

**D19. The input path stays on dedicated threads inside the daemon.** IPC is the control plane only; no input event crosses zenoh. The fixes for the mode-change and rumble-change delays (the bugs above) land in phase 2, by replacing "poll shared settings after a timeout" with a command channel the loops select on. Whether to read evdev directly instead of through gilrs (#10) is measured, not assumed: phase 2 adds a latency bench using the uinput fakes, and the result goes to `docs/ideas/` or a follow-up.

**D20. Configuration becomes daemon-owned profiles.** Today's `config.toml` (the tray's last-used settings) is migrated on first run into a default profile, with a `schema` version field. Named profiles covering multi-pipeline setups are the answer to #6. The file stays at `$XDG_CONFIG_HOME/ctrlassist/`.

### GUI (phase 5)

**D21. Slint, following `tisvlay-gui` / `tisvlay-gui-app`,** with gettext i18n and gamepad-first navigation (TisVlay's couch-mode work applies directly: this app's users are holding controllers). Scope is settled in its own plan.

## Steps

Every commit passes `just check` once phase 1 step 3 lands. "Hardware" means two physical controllers, one with rumble, plus Steam for the steam-hide path.

### Phase 0: repository move

Maintainer actions, in order:

1. Open a tracking issue for this plan and link it from the plan PR.
2. Transfer `ruffsl/CtrlAssist` to the `CtrlAssist` org. Don't create a new repo of that name under `ruffsl` afterward.
3. In the org repo: enable private vulnerability reporting and Discussions (already on), add a ruleset on `main` requiring the `CI result` check once phase 1 creates it, enable Dependabot, add `FUNDING.yml`.
4. Update the local remote (`git remote set-url origin git@github.com:CtrlAssist/CtrlAssist.git`).

Then one small PR:

5. `chore: move to the CtrlAssist org`: `Cargo.toml` `homepage`/`repository`/badges, README links and badges, metainfo URLs and screenshot links. Rename the Flatpak files and IDs to `io.github.ctrlassist.CtrlAssist` (manifest, desktop file, metainfo, icon install path, `release.yml`, `flatpak/README.md`).
   - Verify: `flatpak-builder` builds and `flatpak-builder-lint appstream` passes on the renamed metainfo; `grep -rn ruffsl` leaves only the author field, `FUNDING.yml` and the developer name.

### Phase 1: scaffolding

1. `docs: add the plans convention and this plan` — `docs/plans/README.md` adapted from TisVlay, this file listed under Active plans.
2. `build: pin the toolchain, add the dev shell tools` — exact `rust-toolchain.toml`; `flake.nix` rewritten on TisVlay's (hand-rolled `eachSystem`, `flake-compat`, `default.nix` shim, the generic tool list; `devShell` → `devShells.default`); `.envrc` sourcing `.envrc.local`; `.gitignore`. Keeps `jstest-gtk`, `linuxConsoleTools`, `flatpak-builder`, `appstream`, `librsvg`.
   - Verify: `nix develop -c cargo build`; `direnv reload` gives `just`, `prek`, `cargo nextest`.
3. `build: add the justfile and prek hooks` — `justfile` with the `check` group (`fmt`, `build`, `clippy`, `doc`, `shear`, `typos`, `actionlint`, `zizmor`, `test`, `check`) and `ci` group (`audit`, `hack`, `msrv`); `.config/{clippy,nextest,typos,tombi,actionlint}.toml`; `.cargo/config.toml`; `prek.toml` with hook ids equal to recipe names and the conventional-commit `commit-msg` hook. `deny` and `licenses-html` wait for phase 3. Fix what the new gates flag (typos, formatting, unused deps: nothing in `src/` references `futures` or `uuid`, so `cargo shear` should drop both).
   - Verify: `prek run -a` green.
4. `ci: replace the pipeline` — `ci.yml` (`changes`, `checks`, `ci`, `msrv`, `features`, `CI result`), `pr-title.yml`, `audit.yml`, `test-report.yml`, `dependabot.yml`; all actions SHA-pinned, `persist-credentials: false`. Coverage upload and the `docs` job are stubbed out until there are tests and a book. The existing `release.yml` stays untouched until phase 3.
   - Verify: a draft PR runs only `checks`; marking it ready runs the rest; `zizmor` and `actionlint` clean.
5. `docs: contributing guide, security policy, templates` — `.github/CONTRIBUTING.md`, `SECURITY.md`, PR template, issue forms (bug form asks for version, install method, controllers, distro and desktop, `RUST_LOG=debug` output).
6. `docs: developer docs skeleton and CLAUDE.md` — `docs/conventions.md` (D10), `docs/build.md` (lint policy, what runs when), `docs/architecture.md` describing the code *as it is today*, `docs/testing.md` stub, `docs/{ideas,concerns,bugs}/README.md` indexes, `.claude/CLAUDE.md`. Open issues this plan doesn't close get an ideas entry: #5 spoofing, #13 SteamOS, #17 custom modes, #19 device permissions, #53 control splitting. Known limitations from the README move to `docs/concerns/` or `docs/bugs/`; the likely bugs listed above get `docs/bugs/` entries.
7. `chore(vscode): tasks through just` — `tasks.json`, `launch.json` building with `--profile test`.

### Phase 2: workspace, conventions, tests

Behavior-preserving except for the listed bug fixes. No release, but verified on hardware at the end as if it were one.

1. `refactor: move the crate into a workspace` — root `Cargo.toml` becomes the workspace (`[workspace.package]`, `[workspace.dependencies]`, profiles); the code moves unchanged to `crates/ctrlassist/`. Pure move so the diff reads as renames.
2. `refactor(core): extract vocabulary and the PadState seam` — `ctrlassist-core` with the enums (dropping their `clap` derives in favor of `ValueEnum` impls or wrappers in the CLI crate; serde names unchanged so `config.toml` still loads) and `PadState` (D8).
3. `refactor(core): move the mux modes onto PadState` — with unit tests per mode and proptest invariants, in the same commit.
4. `refactor(core): move the demux modes onto PadState` — same.
5. `refactor(core): move EffectManager and the VDF edit` — the blacklist edit becomes `fn(&str, …) -> Result<String>` with tests over real `config.vdf` shapes (key present, key absent, brace on the same line, empty value); file locking and the atomic write stay in the device crate.
6. `refactor(device): extract ctrlassist-device` — discovery, virtual pads, FF devices, hiders, loops, `start_mux`/`start_demux`. Typed `thiserror` errors replace `Box<dyn Error>`.
7. `refactor(tray): extract ctrlassist-tray` — restructured on `tisvlay-tray`'s shape: menu items send a `TrayCommand` over a channel to an async processor, state is a pure view struct, `ksni::Handle::update` pushes changes. Split `app.rs` into `tray.rs`, `state.rs`, `notify.rs`, `service.rs`. Tests for the view state (which items are enabled when running, name matching from saved config).
8. `feat(runtime): tracing and journald logging` — `ctrlassist-runtime` with `logging::init`; `log` and `env_logger` removed; every message converted to structured fields; the CLI's user-facing lines go through `writeln!`.
9. `test: uinput fake gamepads and end-to-end mux/demux tests` — `ctrlassist-test` (D9) and `tests/it/` in the device crate covering each mode, spoofing identity, FF upload/play/erase round trip, and shutdown.
10. One commit per bug from the list above, each starting from a failing test: mode change applies while input is flowing; rumble change applies without waiting for an FF event (command channel replaces settings polling, D19); device clone and start failures return errors; config read has no side effects.
11. `build: adopt the workspace lint table` — as a short series, one lint or small family per commit with its fixes (`print_stdout`/`print_stderr`, `unwrap_used`/`panic`, `missing_debug_implementations`, `unreachable_pub`, the style lints), then `[lints] workspace = true` everywhere and `CARGO_BUILD_WARNINGS=deny` in the gates.
12. `ci: coverage and the ratchet` — `just test-ci`, `cov*` recipes, Codecov, baseline set at the measured value.
13. `test(bench): input latency through the uinput fakes` — a script or bench plus a results note in `docs/`, answering D19's gilrs question with numbers.
14. `docs: architecture and testing guides for the new layout`.

Verify on hardware: each mux mode and demux mode; `--hide steam` adds and removes the blacklist entry; `sudo … --hide system` hides and restores; `--spoof primary`; rumble to each target; tray start/stop/live mode change; unplug a pad mid-session and confirm an error, not a panic; an existing 0.4.0 `config.toml` loads.

### Phase 3: packaging and release

Blocked on D12 and D13.

1. `build(licensing): cargo-deny and cargo-about` — `packaging/licensing/{deny.toml,about.toml,about.html.hbs,NOTICE}`, the `deny` and `licenses-html` recipes and hooks, `docs/licensing.md`.
2. `feat(cli): shell completions` — clap_complete's dynamic engine; controller IDs complete live. `just completions`.
3. `build(nix): package and overlay` — `packaging/nix/package.nix` (`buildRustPackage`, fileset allow-list, completions, desktop item, udev rule), `packages.default`, `packages.msrv-rust`, `checks.tests`.
4. `build(nix): NixOS module and VM test` — `programs.ctrlassist.enable` (package, udev rule, uinput module load; a user-unit option arrives in phase 4); `packaging/nix/test.nix` runs the uinput integration tests and a CLI smoke test in a VM as `checks.vm`.
5. `build(packaging): deb, rpm and tarball` — nfpm metadata on the `ctrlassist` crate (binary, desktop file, metainfo, icon, udev rule, licenses, completions; `depends` under `overrides.<packager>`), `package-*` recipes, a `package-tarball` recipe, `packaging/README.md` with the release checklist.
   - Verify: install the deb on Ubuntu and the rpm on Fedora in containers; on one real machine of each, run a mux.
6. `ci: release pipeline` — `build-packages.yml` (build, attest), `release.yml` (verify, draft release with checksums; `publish-crates` only if D13 keeps crates.io), `sbom`, `verify-assets` and `verify-release` recipes, `.config/release-plz.toml`, `release-bump`. The Flatpak job is kept and moved to `packaging/flatpak/` or removed, per D12.
7. `docs(book): user guide` — `docs/book/` with pages migrated from the README (D14), `.config/.readthedocs.yaml`, `docs-*` recipes, the CI `docs` job.
8. `feat(docgen): generated CLI reference` — `ctrlassist-docgen`, `docs-gen`, `docs-gen-check`.
9. `docs: slim the README`.
10. `chore(release): 0.5.0`, tag, verify with `just verify-release v0.5.0`, publish the draft.

Maintainer setup this phase needs: Read the Docs project, Codecov, the `crates-io` environment and trusted-publisher entry (if D13 keeps it).

### Phase 4: daemon, protocol, clients (outline)

Own plan doc when it starts. Expected order:

1. `ctrlassist-protocol`: key space, payloads, endpoint table, `WireError`, session bootstrap; `docs/architecture.md` gains the key space and the zenoh laws that apply.
2. `ctrlassist-daemon`: the reactor (devices, pipelines), hotplug handling, pipeline lifecycle with degraded and re-bind states, profile store with migration from `config.toml` (D20).
3. `ctrlassist-client`: `DaemonClient` and `StateWatcher`.
4. CLI: `service daemon`, `status`, `list` and `watch` against the daemon, `pipeline` verbs, and `mux`/`demux` re-based on D16 with unchanged flags.
5. Tray as a client; one icon for all pipelines.
6. Packaging: systemd user unit, module option, the Flatpak's in-process mode if D12 kept it.
7. Wire tests on temp sockets (TisVlay's layer), VM test extended to the service.

Verify on hardware: the phase 2 list again through the daemon, plus unplug and replug mid-pipeline, and the README cookbook setups as daemon requests.

### Phase 5: GUI (outline)

Own plan doc. `ctrlassist-gui` (Slint UI, gettext domain) and `ctrlassist-gui-app` (state, rpcs, binary), D-Bus activation and desktop entry, controller navigation, i18n workflow, packaging of the Slint runtime libraries. The last commit of this phase retires this plan into `docs/`.

## Open questions for review

1. **D12** Flatpak: keep (recommended) or drop?
2. **D13** crates.io: keep publishing (recommended) or stop at 0.4.x?
3. **D17** zenoh for the control plane, or a lighter IPC given there is one user and one socket?
4. **D16** Is keeping `mux`/`demux` as foreground verbs worth the in-process daemon mode, or should 0.6.0 break the CLI cleanly?
5. Should phases 4 and 5 wait for user feedback on 0.5.0 before their plans are written?

## Deviations

None yet.
