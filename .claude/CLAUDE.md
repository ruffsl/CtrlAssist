# CtrlAssist - controller assist for gaming on Linux (Rust)

## Project overview

CtrlAssist lets several physical game controllers act as one virtual controller, or one as several, so a second person can help someone play. It reads pads through the Linux input subsystem, routes their events through a graph, and writes the result to virtual pads that games read instead.

The project is being rewritten from a clean slate. [docs/architecture.md](../docs/architecture.md) is the design; [the plan](../docs/plans/tisvlay-template-overhaul.md) holds the decisions and the numbered build steps. Read the plan's State section first: it says which build step is next.

## Current structure

```
Cargo.toml                    # workspace root: one version, the lint table, the profiles
crates/
  ctrlassist/                 # the client SDK: a daemon client that checks the daemon's
                              # release and mirrors its state, and a state watcher
  ctrlassist-core/            # the routing model: controls, pad state, groups, mux and
                              # demux nodes, the graph, the feedback router. No dependencies.
  ctrlassist-cli/             # the ctrlassist binary: `service daemon`, `service tray`, `status`, `pads`,
                              # `identities`, `identify`, `forget`, `profile`, `start`,
                              # `stop`, `update`, `reload`, `select`, `setups`; shell completions
                              # that ask the daemon for names
  ctrlassist-daemon/          # the control side: the session, liveliness, the reactor
                              # that owns the known pads, the profiles and the setups
                              # and serves the rpcs, the hotplug watch, the stores on
                              # disk, logging
  ctrlassist-device/          # the kernel side: pads read from descriptors, the kernel
                              # gamepad layout, the controller database and its pinned
                              # snapshot, uinput virtual pads, the input thread, hotplug
  ctrlassist-docgen/          # writes the user guide's generated pages (the CLI
                              # reference) from the code they document
  ctrlassist-gui/             # the settings window's compiled Slint UI (ui/*.slint):
                              # tokens, theme, widgets and the shell (lib name
                              # ctrlassist_gui_ui)
  ctrlassist-gui-app/         # the code that drives the window, its tests and the
                              # ctrlassist-gui binary (lib name ctrlassist_gui)
  ctrlassist-hider/           # the hiding service: its wire types and client as a library,
                              # and the binary, which runs as a user of its own
  ctrlassist-icons/           # freedesktop icon-theme lookup: a name, with fallbacks, to
                              # a file in the desktop's theme
  ctrlassist-protocol/        # what the daemon and its clients agree on: the profile
                              # schema; session configuration, keys, the rpc table,
                              # typed wire errors and the version check
  ctrlassist-tray/            # the system tray: the menu as plain data from the SDK's
                              # mirror, each profile's fields in it, notifications from
                              # what changes, and the ksni item that renders them
  ctrlassist-test/            # fake pads made through uinput and uhid, one with rumble
                              # motors, and the
                              # ctrlassist-fixture binary the VM checks drive,
                              # ctrlassist-bench, which times the input path, and
                              # ctrlassist-sdl-probe (feature `sdl-probe`), which says
                              # what SDL3 makes of a pad
vendor/                       # third-party crates carried with a fix upstream lacks;
                              # vendor/README.md lists them
spikes/                       # throwaway workspaces, each deleted by the build step that
                              # replaces it; excluded from the workspace
docs/
  architecture.md             # the design, and what the platform was observed to do
  build.md                    # gates, toolchain, lint policy, dependency review triggers
  testing.md                  # the test layers, the fake pads, the VM checks
  conventions.md              # code conventions (imported below)
  book/                       # the user guide, an mdBook; src/generated is written by
                              # `just docs-gen`
  plans/                      # the plan in flight
  concerns/, ideas/           # one doc per item; each README.md is its index
  gui/, kde/                  # the settings window's theme and key navigation, and
                              # what KDE was seen to do when it is launched
packaging/
  nix/                        # package.nix and module.nix (CtrlAssist as installed on
                              # NixOS), crate.nix (one crate's binaries, for the VM
                              # checks' fixture), the VM checks (test-*.nix), and the
                              # pinned tools: slint-lsp, slint-viewer, cargo-about
  systemd/                    # the hiding service's socket and service units, the
                              # daemon's and the tray's user units, and the session
                              # bus service file that starts the settings window
  freedesktop/                # the settings window's desktop entry
  udev/                       # the rule that gives the seat user uinput and uhid
  scripts/, modules-load/     # what the deb, rpm and Arch packages run at install and
                              # removal, and the modules they have loaded at boot
  licensing/                  # deny.toml (license policy and the one-version gate),
                              # about.toml and its template (the third-party license
                              # bundle the packages ship), and NOTICE
.vscode/tasks.json            # editor tasks that start, stop and restart the dev stack
.claude/CLAUDE.md             # this file
.claude/slint.md              # Slint and its MCP server: what was learned building the
                              # window, TisVlay's notes; read before working on ui/*.slint
.mcp.json                     # the window's Slint MCP server, which `just gui-start` opens
```

The crates still to come, and what each holds, are in the architecture's Crates table.

## References

- **The GUI plan** ([docs/plans/gui.md](../docs/plans/gui.md)) holds the settings window's decisions and build steps; its State section says which is next.
- **The plan** is the source of truth for decisions and order of work. When work departs from it, record what changed and why in its Deviations section, in the same commit.
- **TisVlay** ([github.com/TisVlay/TisVlay](https://github.com/TisVlay/TisVlay), usually checked out at `~/ws/lgtv/orlgtv`) is the template for tooling, lint policy, CI, docs layout, release flow and code conventions. Stay aligned with it wherever that makes sense, so an improvement in one project carries to the other: copy from it and trim, don't re-derive or restyle, and name the TisVlay commit copied from in the commit message. Its `.claude/CLAUDE.md`, `docs/conventions.md`, `docs/build.md` and `docs/testing.md` say how things are done there.
- **The legacy version** (0.4.0) is at the `v0.4.0` tag and the `legacy/0.4` branch. It is a reference for what users could do, not a compatibility target and not a design to follow. Build it from a `git worktree`; it carries its own flake.

## How changes land

Anything beyond a small fix follows a plan in `docs/plans/` ([convention](../docs/plans/README.md)). Plan revisions are committed as they are made, so they read as diffs. The [contributing guide](../.github/CONTRIBUTING.md) has the same for contributors.

## Working rules

- Commit coherent checkpoints with [Conventional Commits](https://www.conventionalcommits.org/) subjects. No `Co-Authored-By` trailer. Never amend or force-push; fixes are new commits.
- Run `prek run -a` before committing; it runs every gate. Outside the dev shell, prefix project commands with `nix develop -c`. For a docs-only change, `prek run -a --skip build --skip clippy --skip doc --skip test`.
- No backward compatibility with 0.4.0: no config migration, no CLI aliases, no shims.
- No plan step or spike numbers in code comments or durable docs; they go stale.
- The justfile and the workflows follow TisVlay, bash included: sharing patterns between the two projects is worth more than a different shell here. No standalone script files for tooling; logic that outgrows a recipe becomes a Rust binary. The maintainer's own shell is fish, so a command suggested for them to type should work there.
- Ask before launching agents.
- Stop for the maintainer's review each time a spike finding is written.
- A new timeout or grace period is a design smell: look for the state or event that makes it unnecessary, and ask if none does.
- American English. Markdown isn't hard-wrapped: one paragraph per line.

## Terms

One word per thing, in code, docs and messages. The table is in [the architecture](../docs/architecture.md#terms). The ones most often got wrong: a physical controller is a *pad*, never a "controller" or "device" outside crate names; the system service that hides pads is the *hiding service*, and *assist* is the helping player's pad.

@../docs/conventions.md

## Tasks and commands (`just`)

All project commands live in the `justfile` at the repo root: prek hooks, editor tasks and CI dispatch through it, and a prek hook's id is the recipe it runs. Run `just` to list the recipes. Every tool a recipe calls comes from the flake's dev shell. A recipe, hook, workflow or tool is added in the commit that first gives it something to do.

After a `flake.nix` change, run `direnv reload` before relying on the shell.
