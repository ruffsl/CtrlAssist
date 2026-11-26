# The user guide, after the settings window

## State

- **Status:** drafted, and its first questions answered (see Decided). Nothing in the book has been changed.
- **Next action:** the maintainer's go-ahead on the plan as a whole; then step 1.
- **Branch:** `docs/user-guide`, from `main` at `39c1b4c`.

## Goal

The user guide was written on 2026-11-21, before the settings window existed and before the maintainer had steered the rewrite toward TisVlay's ways. It leads with the tray and the command line, has no page for the window, no screenshots and no FAQ, and its Profiles page is half a walk through what a profile does and half the reference for a file's keys. TisVlay's guide went through the same growth and was reshaped twice; this plan brings CtrlAssist's to where TisVlay's ended, and adds what TisVlay never had: screenshots made by a command, from made-up pads and profiles, so they can be made again whenever the window changes.

## Where the shape comes from

TisVlay's guide was planned in `docs/plans/mdbook-docs-site.md` and brought back in line with its code by `docs/plans/docs-sweep.md`, both retired on 2026-11-01 (TisVlay `1504d0bb` and `409eabc1`; read them with `git show 1504d0bb^:docs/plans/mdbook-docs-site.md`). What they decided, and why, in the order it was learned:

- **A guide apart from the developer docs.** Everything under `docs/` but the book is for people changing the code. The guide is for someone who has never seen the project, published where a search finds it, and kept under `docs/book/` so the two never mix.
- **mdBook on Read the Docs.** One Rust binary from nixpkgs, with no Python toolchain to add; the README stays short and points at the guide.
- **What can be generated is.** The CLI reference comes from the clap tree and the release version from the workspace manifest, each behind a gate that fails CI when it is stale. A hand-copied flag or default is wrong within a few merges.
- **One home per fact.** The guide grew by accretion: each fact was added where it was first needed and restated wherever a reader might look, until one change in behavior needed edits on seven pages and the pages disagreed. Each fact now has one page; elsewhere a clause says what the reader needs at that point, and links.
- **Leave out what drifts.** Counts beside the list they count, a third party's menu paths, lists of models tried, build baselines, and what a retired feature used to do.
- **The app first.** The last reshaping (2026-10) was for readers new to Linux: the guide leads with the window and the tray, describes each task by the labels they show, and puts the command line and the files under Advanced. A terminal appears on the main path only inside a collapsed "From a terminal" aside, where a terminal really is the fix.
- **Links are checked.** `just docs-links` builds the book and checks every local link and anchor, in CI.

CtrlAssist's `docs/book/README.md` already carries most of these as rules, copied from TisVlay's. It does not say why, and neither does TisVlay's, its plans having been retired without leaving the reasons behind. Step 1 writes them into both.

## Where CtrlAssist's guide departs

Found by reading the two side by side on 2026-11-25. Each is corrected by a step below unless marked as kept.

| | TisVlay | CtrlAssist today |
|---|---|---|
| What leads | The window, then the tray | The tray, then the command line; the window is not described anywhere |
| Sections | Getting Started, Using, Help, Advanced, Reference | Getting Started, Using, Help, Reference; Command Line sits under Using |
| Files | A Configuration Files page under Advanced holds the schema; the pages before it say what things do | Profiles holds both: presets and identities, then the full form, hand edits and where files live |
| Screenshots | Fourteen, under `docs/screenshots/`, in LFS, linked into the book through `src/docs/` | None; no `docs/screenshots/`, no links |
| README | A Screenshots section, and an `overview` anchor the book's introduction includes | Neither; the introduction repeats the README by hand |
| FAQ | A page under Reference | None |
| Terminal asides | Collapsed `<details>` where a terminal is the fix | The rule was copied without that half |
| Writing rules | Say the guide is published | Still say it isn't |
| Generated pages | CLI reference, profile defaults, release version | CLI reference and release version. **Kept:** CtrlAssist's presets take no overrides, so there is no table of defaults to generate |
| Vocabulary | The project's own words | "controller" for the code's "pad", with a table of words on the introduction. **Kept:** it is CtrlAssist's own rule, and a good one |

## Decided

By the maintainer, 2026-11-25:

- **TisVlay's shape,** with the window and the tray on one page.
- **Screenshots are made by a Rust binary** that builds the real window against made-up state and renders it off screen with Slint's software renderer. No compositor, no daemon, no hand at the mouse.
- **The tray's screenshots come last,** being drawn by the desktop and not by CtrlAssist.
- **The reasons above go into `docs/book/README.md`,** and into TisVlay's on its local branch.
- **Screenshots are committed through git LFS,** as the artwork is.

Later the same day, on the questions this plan first left open:

- **Dark for every shot, and one in light,** as TisVlay's are: dark Breeze with its default accent, and one page again in light to show the window follows the desktop.
- **At the size the window opens at,** at a scale of one.
- **The made-up state as proposed:** a DualSense and an Xbox pad connected, an 8BitDo remembered, `couch` running and `desk` stopped.
- **The FAQ's first questions** are of three kinds. Games, Steam and Proton: does the game need to support it, does it work with Steam Input and Proton, why does my game see two controllers. Controllers and latency: which controllers work, does it add latency, do the gyro and the touchpad carry over. Setup and safety: does it need root, where are my profiles kept, can two people each have a virtual controller. Nothing on coming from 0.4.0.

## Contents

```
Introduction                      includes the README's overview

Getting Started
  Installation
  Quick Start                     from the window, with the tray beside it

Using CtrlAssist
  GUI & Tray                      new: the window's pages, couch mode, the tray, notifications
  Profiles                        what a profile does, by the window's labels

Help
  Troubleshooting

Advanced
  Command Line                    moved from Using
  Configuration Files             new: the keys, the full form, hand edits, reload, where files live

Reference
  How It Works
  Requirements
  Security & Supply Chain
  FAQ                             new
```

File paths stay where they are but for `guide/tray.md`, which becomes `guide/gui-tray.md`, and the two new pages, `guide/configuration.md` and `reference/faq.md`.

Canonical homes that change:

| Fact | Was | Becomes |
|---|---|---|
| The tray's menu, its notifications and their switches | The Tray | GUI & Tray |
| The window's pages, key and gamepad navigation, couch mode | nowhere | GUI & Tray |
| Presets, naming a controller, identities, managed profiles, changing a running setup | Profiles | Profiles |
| A profile file's keys, the full form, hand edits and `reload`, where profiles live | Profiles | Configuration Files |
| Motion and the touchpad, the assist's Gyro button | Profiles | Profiles for what it does, Configuration Files for the keys |

## Screenshots

**The tool.** A binary, `ctrlassist-screenshots`, in a crate of its own beside `ctrlassist-docgen` or as a second binary of it; step 2 decides which by what it costs the docs job to build. It links the window's library, builds an `AppWindow` on a platform of its own with Slint's software renderer, fills it as the window's tests fill theirs, and for each shot sets the page and the selection, renders to a buffer and writes a PNG. `just docs-screenshots` runs it and writes `docs/screenshots/`.

**Made-up state.** One set, in the tool, shared by every shot so the screenshots agree with each other: a DualSense and an Xbox pad connected, one more remembered, a `couch` profile running in priority with the DualSense hidden, a `desk` profile stopped, a virtual pad, every part of About at one release. No real serial, address or user name appears; the addresses are the tests' own.

**The shots,** to start with:

| File | Shows |
|---|---|
| `pads_pane.png` | The Pads page with a pad in a running profile: status, Visibility, its profiles |
| `pads_pane_virtual.png` | The same page on a virtual pad |
| `profiles_pane.png` | A profile's form: Mode, Identity, Rumble, Gyro, Visibility, Managed |
| `profiles_pane_running.png` | A running profile, with who has control |
| `settings_pane.png` | Notifications and couch mode |
| `about_pane.png` | The parts and their releases |
| `pads_pane_light.png` | The Pads page again, in the light theme |
| `couch_mode.png` | The window in its ten-foot layout |
| `system_tray_menu.png`, `system_tray_hover.png` | The tray, last, by whatever means step 7 finds |

**What isn't known yet,** and step 2 finds out before anything is built on it:

- Whether the software renderer draws the window closely enough to the one on screen: its text, the icon theme's glyphs, shadows and rounded corners.
- Which fonts and icon theme it finds in the dev shell, and so whether a screenshot made on another machine matches. They need not match byte for byte; they must not look like another application.
- Whether the software renderer can be a feature only this tool turns on, so the shipped window is built as it is today.

**Not a gate.** Pixels differ with the fonts installed, so nothing in CI compares or regenerates them. `just docs-links` already fails on a page that links a screenshot that isn't there.

**In the tree.** `docs/screenshots/*.png` through LFS (`docs/.gitattributes` already tracks `*.png`), with `docs/book/src/docs/screenshots` and `docs/book/src/docs/artwork` as symbolic links to them, as TisVlay has, so a page links `../docs/screenshots/pads_pane.png` and the README links `docs/screenshots/pads_pane.png`.

## Steps

Each is one commit or a few, with `prek run -a --skip build --skip clippy --skip doc --skip test` and `just docs-links` before it, and the whole gates where Rust changes.

1. **The reasons.** `docs/book/README.md` gains "Who the guide is for and why it is shaped this way" from the section above, loses "It isn't published anywhere yet", and gains the terminal-aside rule and, once step 2 has settled them, the screenshot rules. The same section goes into TisVlay's `docs/book/README.md` on `chore/from-ctrlassist`.
2. **The capture tool, one shot.** The binary, the made-up state and `pads_pane.png` alone. **Stop:** the maintainer looks at it beside the real window and says whether the software renderer will do. If it won't, the fallback is the real window under a headless compositor, and this plan is revised before going on.
3. **The rest of the window's shots,** `just docs-screenshots`, the links under `docs/book/src/docs/`, and a line in `docs/build.md` and `docs/testing.md`.
4. **The new contents, without new prose.** `SUMMARY.md` in the order above; Command Line moves; `guide/tray.md` becomes `guide/gui-tray.md`; Profiles' file half moves to `guide/configuration.md`; every link and anchor re-pointed; the README gains its `overview` anchor and the introduction includes it. The book reads as before, in a new order.
5. **GUI & Tray, and Quick Start from the window.** The window's pages by their labels, with their screenshots; couch mode and navigating by keys and gamepad, from `docs/gui/`; the tray after. Quick Start leads with the window.
6. **Profiles by the window's labels,** Configuration Files finished, the FAQ, the README's Screenshots section, and the canonical homes table in `docs/book/README.md`.
7. **The tray's shots.** Tried in this order: a nested Plasma session driven over D-Bus; the menu as the tray's own data, drawn by a small page of the tool; by hand. Its own stop, since each costs differently.
8. **A read of every page against the code,** as TisVlay's sweep was: each label quoted is the label shown, each command runs, each claim holds. Then the plan is retired: what lasts is already in `docs/book/README.md` after step 1.

## Open questions

For the maintainer:

1. **The capture tool for TisVlay** (after step 3). It would replace fourteen screenshots taken by hand there. Carried back as an idea, or as code?

## Deviations

None yet.
