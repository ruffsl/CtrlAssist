# Writing the user guide

The user guide is this mdBook, published on [Read the Docs](https://ctrlassist.readthedocs.io/). Everything else under `docs/` is for developers. `just docs-serve` previews it live, and `just docs-links` checks its links and anchors. After TisVlay's.

## Who it is for, and why it is shaped this way

The rules below were each learned by getting it wrong first. The reasons are here so a rule isn't dropped by someone who doesn't know what it prevents.

- **It is for someone who has never seen the project,** and may be new to Linux. Everything else under `docs/` is for people changing the code. Keeping the guide apart, under `docs/book/`, and published where a search finds it, lets each be written for its reader, and keeps the README short.
- **It leads with the app.** A reader who wants something done looks for a window to do it in. So the guide walks through the window and the tray first, by the labels they show, and puts the command line and the files under Advanced. A terminal on the main path loses the reader the guide most needs to keep.
- **What can be generated is.** A flag, a default or a release number copied by hand into prose is wrong within a few merges and nobody notices. Each is written from the code behind a gate that fails CI when it is stale.
- **One home per fact,** because the guide grows by accretion: each fact is added where it is first needed and then restated wherever a reader might look. In time one change in behavior needs edits on several pages, some are missed, and the pages disagree.
- **What drifts is left out,** for the same reason: a count beside the list it counts, another project's menu path, a list of models tried. Each is true the day it is written and unowned after.
- **Links and anchors are checked** in CI, since moving a fact to its one home is done by linking to it, and a guide of links is only as good as its links.

## Rules

- **Written for someone who has never seen the project.** It says "controller" where the code and the developer docs say "pad", and the table of words on the introduction is the guide's own vocabulary.
- **One canonical home per fact.** Elsewhere, keep only the consequence the reader needs at that point, in a clause, plus a link.
- **The CLI reference and the release version are generated,** into `src/generated/` behind a drift gate (`just docs-gen-check`). Don't hand-copy a flag into the prose.
- **Describe tasks with the labels the tray and the window show,** taken from `crates/ctrlassist-tray/src/menu.rs` and `crates/ctrlassist-gui/ui/`, and keep implementation vocabulary (sockets, keys, rpcs) off the main path. When a label changes, grep the book and `.github/` for the old one.
- **Command-line equivalents live in one of two places:** the "Which command does what the tray does" table on the Command Line page, or a collapsed "From a terminal" `<details>` aside where a terminal really is the fix.
- **Leave out what drifts:** counts beside the list they count, third-party menu paths, tested-model lists, and references to retired features.
- **Say what has been tried, and on what.** Only NixOS with KDE Plasma has been used for real; the guide doesn't promise more.

## Canonical homes

| Fact | Page |
|---|---|
| The presets, naming a controller, identities, managed profiles, the full form, updating a running setup, hand edits and `reload` | Profiles |
| The tray's menu, its notifications and their switches | The Tray |
| What each command does from a terminal | Command Line |
| What the packages and the NixOS module install, and removing them | Installation |
| The pieces, hiding, latency, layouts, and where things are kept | How It Works |
| What it runs on, what has been tried, and compatibility between releases | Requirements |
| What runs with privilege, and checking a release (signatures, checksums, SBOMs) | Security & Supply Chain |

## Tooling

- **Why mdBook:** it's a single Rust binary in nixpkgs, with no Python toolchain to add. Reconsider it if the guide ever needs versioned or translated docs.
- **A new preprocessor** has to be installed in three places: `flake.nix`, the `docs` job in `.github/workflows/ci.yml`, and `.config/.readthedocs.yaml`. Keep their versions in step.
- **The release version partial** comes from the workspace `Cargo.toml`: `just release-bump` regenerates it with the bump, and `just docs-gen-check` fails if it's stale. It assumes `main`'s version equals the latest release, which is untrue from a release PR's merge until that release is published, and untrue now, before 0.5.0's first release.
