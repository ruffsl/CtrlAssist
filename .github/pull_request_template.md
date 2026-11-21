## What and why

<!-- What changes, and the problem it solves. Link the issue if there is one. -->

## Plan

<!-- docs/plans/<name>.md, or "not needed" for a small fix. -->

## Tested on

<!-- Which pads and how connected, which desktop; or the fake pads only. -->

## Checklist

See the [contributing guide](https://github.com/CtrlAssist/CtrlAssist/blob/main/.github/CONTRIBUTING.md).

- [ ] The title follows Conventional Commits, and a change the daemon and its clients must agree on is a `feat` or breaking, never a `fix`.
- [ ] A plan-driven PR: every commit subject follows them too, and the plan records any deviations.
- [ ] `prek run -a` passes, or the docs-only check for a docs change.
- [ ] Tests cover the change, where it applies.
- [ ] Generated docs (`just docs-gen`) are regenerated, if the command line changed.
