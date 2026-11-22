# Contributing to CtrlAssist

CtrlAssist lets several game controllers act as one, so a second person can help someone play. Bug reports, ideas, docs and code are all welcome, and most of the work doesn't need two controllers on your desk: fake pads made through the kernel stand in for them.

## Where things go

| You have | Go to |
|---|---|
| A question | [Discussions](https://github.com/CtrlAssist/CtrlAssist/discussions) |
| A bug or a feature idea | [The issue forms](https://github.com/CtrlAssist/CtrlAssist/issues/new/choose) |
| A security problem | [The security policy](/.github/SECURITY.md); please don't open a public issue |
| A controller that is laid out wrong | An issue with `ctrlassist pads` and what is wrong; a line for `~/.config/ctrlassist/gamecontrollerdb.txt` fixes it locally meanwhile |
| A change bigger than a small fix | An issue first, so the approach is agreed before you plan it ([how changes land](#how-changes-land)) |

## Setup

```bash
git clone https://github.com/CtrlAssist/CtrlAssist.git
cd CtrlAssist
nix develop     # or `direnv allow`
prek install    # the pre-commit and commit-msg hooks
```

The [Nix](https://nixos.org/download/) dev shell provides the Rust toolchain, `just` and every tool the checks use. Without Nix, [the CI workflow](/.github/workflows/ci.yml) shows what CI installs on Ubuntu, which is a starting point.

`prek install` makes every commit run the full build, clippy and the tests. For just the commit subject check, use `prek install -t commit-msg` and run the checks yourself before pushing.

Run each service in its own terminal, each inside the dev shell:

```bash
just hider-dev      # optional, asks for sudo: the hiding service
just daemon-start
just tray-start
just gui-start       # the settings window
```

In VS Code, the `all: start`, `all: stop` and `all: restart` tasks run the stack with a panel per service. They need the dev shell's environment to find `just`, so start VS Code from the dev shell or use a direnv extension.

`just cli <verb>` runs any command against that stack, and `just stop-all` ends it. Run `just` to list every recipe. The dev daemon keeps its profiles and its list of pads under `target/`, apart from an installed daemon's, but it uses the same socket: stop an installed one first (`systemctl --user stop ctrlassist-daemon ctrlassist-tray`).

Tests that make fake pads need `/dev/uinput` to be writable by you, which the packaged udev rule, or Steam's own, arranges. Where it isn't, those tests skip and say so; the virtual machine checks (`just vm <name>`) run them all as root on a clean machine.

## How changes land

Small fixes go straight to a PR. Anything that changes what the daemon and its clients agree on (a key, a payload, an rpc, the profile format), adds a feature, or needs reviewing in stages follows a plan. If you're unsure, ask on the issue; the maintainer decides. If a PR outgrows "small", or you've already written the code, open it anyway and say so: the maintainer will take it as is or ask for a short plan on the same branch.

1. **Agree the approach** in an issue.
2. **Plan.** Draft `docs/plans/<name>.md` ([what it holds](/docs/plans/README.md#what-a-plan-holds)). Commit it and open a draft PR, so later changes show up as diffs.
3. **Get the plan reviewed.** Ask for review on the draft PR and wait for the maintainer's agreement before implementing. Reviewing it yourself or with an agent first is welcome. A review looks at design soundness, implementation correctness (do the plan's claims about the code hold?) and maintainability.
4. **Implement,** one commit per step, each passing the checks, with tests in the same commit as the code. A bug fix starts with a test that reproduces it, where one can. When the work departs from the plan, record what changed and why in the plan, in the same commit. Mark the PR ready when you want the full CI run.
5. **Get the implementation reviewed** with the same lenses. Fixes land as new commits. Whatever the tests can't reach gets tested on hardware; if you don't have it, say what's untested, and the maintainer will test it or find someone who can.
6. **Retire the plan** when the maintainer says the PR is ready to merge, moving what's worth keeping into `docs/` ([how](/docs/plans/README.md#convention)). If you're unsure what to keep, ask.

## Before opening a PR

- Run `prek run -a`. CI also checks the minimum Rust version, coverage and the book. For a docs-only change, `prek run -a --skip build --skip clippy --skip test --skip doc` is enough, plus `just docs-links` if you touched the book.
- If you changed the command line, run `just docs-gen` and commit what it regenerates; CI fails on a stale reference.
- Add tests for what you changed, where it applies. [docs/testing.md](/docs/testing.md) covers the layers, the fake pads and where new tests go.
- Code that runs as root, the hiding service, is tested only in the virtual machine checks: `just vm hider` and `just vm daemon`.

## Commits and PRs

- **The PR title must follow [Conventional Commits](https://www.conventionalcommits.org/),** for example `fix(device): read a hat that reports diagonals`, and CI checks it. release-plz derives the next version and the changelog from the subjects on `main`.
- **A `feat` bumps the minor version,** and the daemon and its clients refuse each other across minor versions. So a change they must agree on is a `feat` or breaking, never a `fix`.
- **Small PRs are squash-merged** under that title, so commits on the branch don't need to be tidy.
- **Plan-driven PRs keep their commits** through a merge commit, so the plan and its history stay in git. Every commit subject on the branch must follow Conventional Commits too, and CI checks that once a commit touches `docs/plans/`. To fix a bad subject, reword it and force-push your own branch before merge, the one exception to fixes landing as new commits. To sync with `main`, merge it in.
- **Draft PRs** run only the fast checks; marking one ready starts the full run. On your first PR, CI waits for the maintainer to approve the run.
- **Say what you tested on:** which pads and how connected, which desktop, or the fake pads only.
- **AI-assisted work is welcome and held to the same standard:** you understand what you submit, you've tested it, and you can answer questions about it. It doesn't need to be disclosed.

## Where to read more

- [docs/conventions.md](/docs/conventions.md): the code conventions the lints can't express. Read this before writing code.
- [docs/architecture.md](/docs/architecture.md): the processes, the routing model, the input thread, hiding, the control plane, and what the platform was observed to do.
- [docs/build.md](/docs/build.md): the gates, the lint policy and what CI runs.
- [docs/testing.md](/docs/testing.md): test layers, the fake pads and the virtual machine checks.
- [packaging/README.md](/packaging/README.md): the packages, the licenses and releasing.
- [docs/ideas/](/docs/ideas/README.md) and [docs/concerns/](/docs/concerns/README.md): deferred work and design concerns, each with an index. Good places to find something to pick up.

## License

CtrlAssist is licensed under [Apache-2.0](/LICENSE). Under section 5 of the license, anything you submit for inclusion is under the same terms, unless you explicitly state otherwise. There's no CLA or sign-off.
