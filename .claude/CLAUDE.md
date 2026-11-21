# CtrlAssist - controller assist for gaming on Linux (Rust)

## Project overview

CtrlAssist lets several physical game controllers act as one virtual controller, or one as several, so a second person can help someone play. It reads pads through the Linux input subsystem, routes their events through a graph, and writes the result to virtual pads that games read instead.

The project is being rewritten from a clean slate. [docs/architecture.md](../docs/architecture.md) is the design; [the plan](../docs/plans/tisvlay-template-overhaul.md) holds the decisions and the numbered build steps. Read the plan's State section first: it says which build step is next.

## Current structure

```
Cargo.toml                    # workspace root: one version, the lint table, the profiles
crates/
  ctrlassist-core/            # the routing model: controls, pad state, groups, mux and
                              # demux nodes, the graph, the feedback router. No dependencies.
  ctrlassist-device/          # the kernel side: pads read from descriptors, the kernel
                              # gamepad layout, uinput virtual pads
  ctrlassist-hider/           # the hiding service: its wire types and client as a library,
                              # and the root binary
  ctrlassist-test/            # fake pads made through uinput and uhid, and the
                              # ctrlassist-fixture binary the VM checks drive
spikes/                       # throwaway workspaces, each deleted by the build step that
                              # replaces it; excluded from the workspace
docs/
  architecture.md             # the design, and what the platform was observed to do
  build.md                    # gates, toolchain, lint policy, dependency review triggers
  testing.md                  # the test layers, the fake pads, the VM checks
  conventions.md              # code conventions (imported below)
  plans/                      # the plan in flight
  concerns/, ideas/           # one doc per item; each README.md is its index
packaging/
  nix/                        # crate.nix (one crate's binaries as a package) and the
                              # VM checks (test-*.nix)
  systemd/                    # the hiding service's socket and service units
  udev/                       # the rule that gives the seat user uinput and uhid
  licensing/                  # deny.toml: license policy and the one-version gate
.claude/CLAUDE.md             # this file
```

The crates still to come, and what each holds, are in the architecture's Crates table.

## References

- **The plan** is the source of truth for decisions and order of work. When work departs from it, record what changed and why in its Deviations section, in the same commit.
- **TisVlay** ([github.com/TisVlay/TisVlay](https://github.com/TisVlay/TisVlay), usually checked out at `~/ws/lgtv/orlgtv`) is the template for tooling, lint policy, CI, docs layout, release flow and code conventions. Stay aligned with it wherever that makes sense, so an improvement in one project carries to the other: copy from it and trim, don't re-derive or restyle, and name the TisVlay commit copied from in the commit message. Its `.claude/CLAUDE.md`, `docs/conventions.md`, `docs/build.md` and `docs/testing.md` say how things are done there.
- **The legacy version** (0.4.0) is at the `v0.4.0` tag and the `legacy/0.4` branch. It is a reference for what users could do, not a compatibility target and not a design to follow. Build it from a `git worktree`; it carries its own flake.

## How changes land

Anything beyond a small fix follows a plan in `docs/plans/` ([convention](../docs/plans/README.md)). Plan revisions are committed as they are made, so they read as diffs. A contributing guide arrives with the first release.

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

One word per thing, in code, docs and messages. The table is in [the architecture](../docs/architecture.md#terms). The ones most often got wrong: a physical controller is a *pad*, never a "controller" or "device" outside crate names; the root process is the *hiding service*, and *assist* is the helping player's pad.

@../docs/conventions.md

## Tasks and commands (`just`)

All project commands live in the `justfile` at the repo root: prek hooks, editor tasks and CI dispatch through it, and a prek hook's id is the recipe it runs. Run `just` to list the recipes. Every tool a recipe calls comes from the flake's dev shell. A recipe, hook, workflow or tool is added in the commit that first gives it something to do.

After a `flake.nix` change, run `direnv reload` before relying on the shell.
