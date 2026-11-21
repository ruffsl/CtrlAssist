# CtrlAssist - controller assist for gaming on Linux (Rust)

## Project overview

CtrlAssist lets several physical game controllers act as one virtual controller, or one as several, so a second person can help someone play. It reads pads through the Linux input subsystem, routes their events through a graph, and writes the result to virtual pads that games read instead.

The project is being rewritten from a clean slate. There is no shipped code on this branch yet: the plan, [docs/plans/tisvlay-template-overhaul.md](../docs/plans/tisvlay-template-overhaul.md), holds the decisions, the spikes and what comes next. Read its State section first.

## References

- **The plan** is the source of truth for decisions and order of work. When work departs from it, record what changed and why in its Deviations section, in the same commit.
- **TisVlay** ([github.com/TisVlay/TisVlay](https://github.com/TisVlay/TisVlay), usually checked out at `~/ws/lgtv/orlgtv`) is the template for tooling, lint policy, CI, docs layout, release flow and code conventions. Copy from it and trim, don't re-derive, and name the TisVlay commit copied from in the commit message. Its `.claude/CLAUDE.md`, `docs/conventions.md`, `docs/build.md` and `docs/testing.md` say how things are done there.
- **The legacy version** (0.4.0) is at the `v0.4.0` tag and the `legacy/0.4` branch. It is a reference for what users could do, not a compatibility target and not a design to follow. Build it from a `git worktree`; it carries its own flake.

## How changes land

Anything beyond a small fix follows a plan in `docs/plans/` ([convention](../docs/plans/README.md)). Plan revisions are committed as they are made, so they read as diffs. A contributing guide arrives with the first release.

## Working rules

- Commit coherent checkpoints with [Conventional Commits](https://www.conventionalcommits.org/) subjects. No `Co-Authored-By` trailer. Never amend or force-push; fixes are new commits.
- Run `prek run -a` before committing. Outside the dev shell, prefix project commands with `nix develop -c`.
- No backward compatibility with 0.4.0: no config migration, no CLI aliases, no shims.
- No plan step or spike numbers in code comments or durable docs; they go stale.
- No standalone shell scripts for tooling or test harnesses: `just` recipes or Rust binaries.
- Ask before launching agents.
- Stop for the maintainer's review each time a spike finding is written.
- A new timeout or grace period is a design smell: look for the state or event that makes it unnecessary, and ask if none does.
- American English. Markdown isn't hard-wrapped: one paragraph per line.

## Terms

One word per thing, in code, docs and messages:

| Term | Meaning |
|---|---|
| pad | A physical controller. Not "controller" or "device", except in crate names. |
| virtual pad | A controller the daemon creates for games to read. |
| source, sink | A pad, or a virtual pad, as it appears in a graph. Input flows source to sink; feedback originates at a sink and flows to sources. |
| node | A transform between sources and sinks. |
| graph | Sources, nodes, sinks and the edges between them. |
| profile | A saved graph with its settings, in a file the daemon owns. |
| preset | A built-in profile shape a client offers by name. |
| setup | A profile that is currently running. |
| hide | Keep games from seeing a pad. A grab is one mechanism for it. |
| identity | The name, IDs and, where emulated, the HID personality a virtual pad presents. |
| feedback | Anything flowing from game to pad: force feedback (rumble is one kind), LEDs, trigger effects. |

## Tasks and commands (`just`)

All project commands live in the `justfile` at the repo root: prek hooks, editor tasks and CI dispatch through it, and a prek hook's id is the recipe it runs. Run `just` to list the recipes. Every tool a recipe calls comes from the flake's dev shell. A recipe, hook, workflow or tool is added in the commit that first gives it something to do.

After a `flake.nix` change, run `direnv reload` before relying on the shell.
