# Plans

Working space for in-progress plan docs. Which changes need one, and the steps from plan to merge, are in the [contributing guide](../../.github/CONTRIBUTING.md#how-changes-land).

## What a plan holds

- The goal, and why it's worth doing.
- The decisions, each with its reason.
- Commit-sized steps, and how each will be verified: by tests, or on hardware where tests can't reach.
- Deviations, added as the work goes: what changed from the plan and why, in the commit that departs from it.

## Convention

- One file per plan, `kebab-case.md`, listed under Active plans below.
- Plans live here while the work is active. In the PR's last commit, migrate the long-term content (decisions worth re-reading, war stories, deferred work pointers) into a durable home in `docs/` and delete the plan file. Plan-driven PRs keep their commits through a merge commit, so git history is the archive.
- Suggested destinations for the typical sections:
  - **Architecture / decisions** → `docs/architecture.md`.
  - **Field-test findings / failure modes** → `docs/concerns/`.
  - **Deferred follow-up work** → `docs/ideas/<topic>.md` with an index entry in `docs/ideas/README.md`.
  - **Process retrospectives / experiment receipts** → fold into the destination doc as a "Lessons" subsection rather than carrying forward as a plan.
- After a plan is migrated, sweep inbound references (`grep -rn 'docs/plans/<name>' --include='*.rs' --include='*.md'`) and re-point them at the new home.

## Active plans

- [`tisvlay-template-overhaul.md`](tisvlay-template-overhaul.md) — the clean-slate rewrite on the TisVlay template: decisions, spikes and their exit criteria, then the build.
- [`gui.md`](gui.md) — the settings window, on TisVlay's GUI: what is copied, the panes, and the build steps.
- [`bluetooth-output.md`](bluetooth-output.md) — whole output reports for a DualSense on Bluetooth: lights and trigger effects, not rumble alone.
- [`user-guide.md`](user-guide.md) — the user guide reshaped after TisVlay's now that there is a settings window, with screenshots a command makes from made-up state.
- [`steam-controller.md`](steam-controller.md) — Valve's new Steam Controller as a pad CtrlAssist reads, and whether it can present one: research, a fake, the build, and an identity behind a spike.
- [`review.md`](review.md) — a workspace review before 0.5.0: every finding triaged by area, the hiding service without root, and the fix batches.

Remove the entry when the plan retires and its file deletes (per Convention above). Git history is the archive, so there is no retired-plans list here.
