# Writing the user guide

The user guide is this mdBook. It isn't published anywhere yet. Everything else under `docs/` is for developers. `just docs-serve` previews it live, and `just docs-links` checks its links and anchors. After TisVlay's.

## Rules

- **Written for someone who has never seen the project.** It says "controller" where the code and the developer docs say "pad", and the table of words on the introduction is the guide's own vocabulary.
- **One canonical home per fact.** Elsewhere, keep only the consequence the reader needs at that point, in a clause, plus a link.
- **The CLI reference and the release version are generated,** into `src/generated/` behind a drift gate (`just docs-gen-check`). Don't hand-copy a flag into the prose.
- **Describe tasks with the labels the tray and the window show,** taken from `crates/ctrlassist-tray/src/menu.rs` and `crates/ctrlassist-gui/ui/`, and keep implementation vocabulary (sockets, keys, rpcs) off the main path. When a label changes, grep the book and `.github/` for the old one.
- **Command-line equivalents live in one place:** the "Which command does what the tray does" table on the Command Line page, unless a terminal really is the fix, as on Troubleshooting.
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
- **A new preprocessor** has to be installed in two places: `flake.nix` and the `docs` job in `.github/workflows/ci.yml`. Keep their versions in step.
- **The release version partial** comes from the workspace `Cargo.toml`: `just release-bump` regenerates it with the bump, and `just docs-gen-check` fails if it's stale. It assumes `main`'s version equals the latest release, which is untrue from a release PR's merge until that release is published, and untrue now, before 0.5.0's first release.
