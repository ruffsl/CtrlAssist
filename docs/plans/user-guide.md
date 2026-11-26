# The user guide, after the settings window

## State

- **Status:** approved by the maintainer on 2026-11-25, after the capture was spiked (see Spike: drawing the window off screen). Building.
- **Next action:** step 1, then step 2 and its stop.
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
| README | TisVlay's stanzas, each headed by an emoji: Features as emoji bullets of two lines each, Screenshots, Architecture with a diagram, Documentation, Acknowledgments; an `overview` anchor the book's introduction includes | Plain headings and long feature sentences; no screenshots, diagram or anchor, so the introduction repeats the README by hand; Installing and License stanzas TisVlay doesn't have |
| Diagrams | mdbook-mermaid, for the architecture diagram | None; the rewrite plan left it out |
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
- **At the size the window opens at.** First at a scale of one; after the spike, at a scale of two: 1440 by 960, as TisVlay's are. A pad's name cut short at that size is what a user first sees.
- **The made-up state as proposed:** a DualSense and an Xbox pad connected, an 8BitDo remembered, `couch` running and `desk` stopped.
- **The README takes TisVlay's stanzas,** as a step of this plan (see The README below), with what is worth keeping of 0.4.0's README harvested into it and into the guide.
- **Background goes to the FAQ:** who CtrlAssist is for, why not pass one controller around, why it was made, and the links to the consoles' own features. The README's opening keeps one sentence naming them.
- **The modes' emojis and examples** from 0.4.0 go into the guide's Profiles page, rewritten for what each mode does now; the README names the modes in one feature bullet.
- **A diagram of the parts, in mermaid,** in the README and so in the guide's introduction: mdbook-mermaid joins the dev shell, the docs job and Read the Docs.
- **The capture tool is a second binary of the window's crate,** behind a feature that turns the software renderer on, so it reaches the window's icon loader and the shipped window is built without it. It sets the color scheme through Slint's internals, at the workspace's exact pin, and the window gains nothing for the tool's sake.
- **No cookbook yet.** 0.4.0's recipes haven't been tried on the rewrite, a virtual pad as another setup's input least of all; filed as `docs/ideas/cookbook.md`, for after this plan.
- **The FAQ's first questions** are of three kinds. Games, Steam and Proton: does the game need to support it, does it work with Steam Input and Proton, why does my game see two controllers. Controllers and latency: which controllers work, does it add latency, do the gyro and the touchpad carry over. Setup and safety: does it need root, where are my profiles kept, can two people each have a virtual controller. Nothing on coming from 0.4.0. Before them, the background above.

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

## The README

TisVlay's README is a fixed run of stanzas, and CtrlAssist's takes the same, in the same order:

| Stanza | Holds |
|---|---|
| Banner, one paragraph, badges | What it is in a sentence or two, with who it is for and the consoles' features it brings to Linux named once. The note that 0.5.0 isn't released stays until it is. |
| ✨ Features | Emoji bullets, each a short line with two shorter lines under it, as TisVlay's are and 0.4.0's were. |
| 📸 Screenshots | A table of the window's pages and the tray, from `docs/screenshots/`. |
| 🏛️ Architecture | A mermaid diagram of the parts, then a table of two columns, component and description. |
| 📖 Documentation | Where the rendered guide is, then emoji bullets by what the reader wants. |
| 🙏 Acknowledgments | As now. |

Everything from the banner to the end of Architecture is inside `<!-- ANCHOR: overview -->`, which the guide's introduction includes in place of repeating it.

**Features,** as a first list for the maintainer to cut: 🎮 two controllers as one (primary and assist); 🎛️ the modes, named, with a link to Profiles; 🔁 one controller as several; 🙈 real controllers hidden while it runs, Steam included; 🕹️ the virtual controller's identity; 🫨 rumble, lights and trigger effects passed back; 🧭 motion and the touchpad; 🔌 a controller that disconnects and returns; 🖱️ the window and the tray; 🛋️ couch mode; ⌨️ the command and profiles as files; ⚡ latency.

**Harvested from 0.4.0** (`git show v0.4.0:README.md`): the emoji for each feature and each mode; the two-line bullet form; the list of who it is for; the FAQ's first three answers and the Background links, into the guide's FAQ; each mode's "ideal for" example, into Profiles.

**Not carried:** the Install, Usage and Configuration stanzas, which are the guide's now; the Limitations, which were 0.4.0's; the screencasts, which show 0.4.0's tray; the crates.io badges; the Cookbook, for the reason under Decided.

**Goes from today's README:** the Installing stanza, whose one paragraph becomes the Documentation stanza's first link, and the License stanza, which the badge and the file already say. TisVlay has neither.

## Screenshots

**The tool.** A binary, `ctrlassist-screenshots`, of the window's crate, built only with its `screenshots` feature. It builds an `AppWindow` on a platform of its own with Slint's software renderer, fills it as the window's tests fill theirs, and for each shot sets the page and the selection, renders to a buffer and writes a PNG. `just docs-screenshots` runs it and writes `docs/screenshots/`.

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
| `system_tray_menu.png`, `system_tray_hover.png` | The tray, last, by whatever means step 8 finds |

**What wasn't known** when this was first written is answered by the spike below: the software renderer draws the window as it is on screen, with the machine's fonts and icons, as a feature only the tool turns on.

**Not a gate.** Pixels differ with the fonts installed, so nothing in CI compares or regenerates them. `just docs-links` already fails on a page that links a screenshot that isn't there.

**In the tree.** `docs/screenshots/*.png` through LFS (`docs/.gitattributes` already tracks `*.png`), with `docs/book/src/docs/screenshots` and `docs/book/src/docs/artwork` as symbolic links to them, as TisVlay has, so a page links `../docs/screenshots/pads_pane.png` and the README links `docs/screenshots/pads_pane.png`.

## Spike: drawing the window off screen

**Question.** Can the settings window be drawn with no display, from made-up state, closely enough to stand as a screenshot?

**Method.** `spikes/screenshots`, a throwaway workspace: a platform of its own around Slint's `MinimalSoftwareWindow`, the real `AppWindow` from the window's library, filled by `Snapshot::show` as the window's tests fill theirs, the Pads page chosen, and `Window::take_snapshot` written out as a PNG. Run in the dev shell on the maintainer's machine, 2026-11-25, Slint 1.19.0.

**Finding: it can.** Three captures of the Pads page, dark, light, and dark at twice the density, each drawn in well under a second:

- **Text, icons and shape are right.** The sidebar's and the buttons' glyphs come from the desktop's icon theme through the window's own loader; chips, rounded cards, the selected row's accent and the health dots are as on screen. Nothing was missing or misplaced.
- **Both themes work,** and so does a scale of two: 1440 by 960 for the window's 720 by 480, the density TisVlay's hand-taken screenshots have.
- **No change to the shipped window.** The software renderer and its use of system fonts are features the spike's own manifest turns on; the workspace's build is untouched.

**What the tool has to work around,** each small:

- **The color scheme is set through Slint's internals:** `i_slint_core`'s context has the process's scheme, which a platform's backend sets and a public platform can't. The spike depends on `i-slint-core` at the workspace's exact Slint pin and sets it there. The alternative is a property of the window that overrides the scheme, which is more code in the shipped window for a tool's sake.
- **The icon loader is the library's own.** The spike compiles the window's `icons.rs` and `icon_theme.rs` into itself by path. The tool wants them reachable: one public function in the window's library, or the tool as a second binary of that crate.
- **Nothing settles by itself.** With no event loop, the spike steps Slint's timers and animations twenty times before it captures. The tool does the same after each change of page or selection.
- **There is no title bar,** the compositor drawing that and not the window. TisVlay's screenshots have one. Without it a capture is the window's contents alone, the same on any desktop.
- **At the size the window opens at, 720 by 480, a pad's name is cut short** in the list and the header, as it is on screen at that size.
- **Fonts and icons are the machine's.** The captures used what the dev shell and the desktop gave: they are not the same bytes on another machine, as the plan expected.

**So.** The capture tool is built as planned, at step 2. The maintainer, having seen the three, chose a scale of two at the size the window opens at; the scheme through Slint's internals; and the tool as a second binary of the window's crate.

## Steps

Each is one commit or a few, with `prek run -a --skip build --skip clippy --skip doc --skip test` and `just docs-links` before it, and the whole gates where Rust changes.

1. **The reasons.** `docs/book/README.md` gains "Who the guide is for and why it is shaped this way" from the section above, loses "It isn't published anywhere yet", and gains the terminal-aside rule and, once step 2 has settled them, the screenshot rules. The same section goes into TisVlay's `docs/book/README.md` on `chore/from-ctrlassist`.
2. **The capture tool, one shot.** The binary, the made-up state and `pads_pane.png` alone, and `spikes/screenshots` deleted. **Stop:** the maintainer looks at it beside the real window.
3. **The rest of the window's shots,** `just docs-screenshots`, the links under `docs/book/src/docs/`, and a line in `docs/build.md` and `docs/testing.md`.
4. **The new contents, without new prose.** `SUMMARY.md` in the order above; Command Line moves; `guide/tray.md` becomes `guide/gui-tray.md`; Profiles' file half moves to `guide/configuration.md`; every link and anchor re-pointed. The book reads as before, in a new order.
5. **GUI & Tray, and Quick Start from the window.** The window's pages by their labels, with their screenshots; couch mode and navigating by keys and gamepad, from `docs/gui/`; the tray after. Quick Start leads with the window.
6. **Profiles by the window's labels,** with each mode's emoji and example; Configuration Files finished; the FAQ, opening with the background; and the canonical homes table in `docs/book/README.md`.
7. **The README in TisVlay's stanzas,** as The README above: mdbook-mermaid in `flake.nix`, the docs job and `.config/.readthedocs.yaml`, their versions in step; the diagram; the Screenshots table; the `overview` anchor widened to the whole of it, and the introduction's own copy of anything it now includes removed.
8. **The tray's shots.** Tried in this order: a nested Plasma session driven over D-Bus; the menu as the tray's own data, drawn by a small page of the tool; by hand. Its own stop, since each costs differently.
9. **A read of every page against the code,** as TisVlay's sweep was: each label quoted is the label shown, each command runs, each claim holds. Then the plan is retired: what lasts is already in `docs/book/README.md` after step 1.

## Open questions

For the maintainer:

1. **The capture tool for TisVlay** (after step 3). It would replace fourteen screenshots taken by hand there. Carried back as an idea, or as code?

## Deviations

None yet.
