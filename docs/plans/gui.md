# A settings window, on TisVlay's GUI

## State

- **Status:** decisions settled with the maintainer on 2026-11-21; the build is under way. Build steps 1 to 4 are done: the window opens with its sidebar and five empty panes, is steered by keys, with couch mode, follows the daemon, and its Reload asks the daemon to reread its files.
- **Next action:** build step 5, the About pane, which brings the icons crate.
- **Branch:** `feat/rewrite`, with the rewrite. Nothing is pushed.
- **Template:** TisVlay at `5ddc2c0d` (`crates/tisvlay-gui`, `crates/tisvlay-gui-app`, `crates/tisvlay-icons`, `docs/gui/`).

## Goal

A window for what the tray's menu is too small for: seeing every known pad and what it is doing, seeing and steering the running setups, and making and editing profiles without writing TOML. Legacy issue #7.

It is TisVlay's GUI with the TV-shaped parts replaced: the same crates, shell, widgets, tokens, key navigation, help, tests and tooling, so a fix in one project carries to the other.

## Decided

Answered by the maintainer on 2026-11-21 unless marked otherwise.

**The first release waits for the GUI.** 0.5.0 ships the daemon, CLI, tray and GUI together. This changes the rewrite plan's "the first release is daemon, CLI and tray", recorded there as a deviation. The TUI still follows later.

**It is built now, on `feat/rewrite`.** The rewrite plan stays open until the release; this plan holds the GUI's decisions and steps, and retires with it or just before.

**Two crates, as TisVlay has.** `ctrlassist-gui` is the compiled Slint UI and owns the gettext domain; `ctrlassist-gui-app` is the state, the rpcs, the tests and the `ctrlassist-gui` binary. The split keeps a Rust change from recompiling the UI. The GUI is its own binary, not a `ctrlassist service` subcommand, so the CLI and the daemon don't link a toolkit. (Follows the template; not asked.)

**Slint, pinned exactly as TisVlay pins it,** with the same features (`backend-winit-wayland`, `renderer-femtovg`, `gettext`, `unstable-winit-030`), `slint-build` and `i-slint-backend-testing` pinned beside it. (Follows the template; not asked.)

**Panes: Pads, Setups, Profiles, Settings, About.** The shell is TisVlay's: a sidebar, and in each pane a list and a detail.

- **Pads:** every known pad with its name, model, how it is connected, whether it is plugged in, whether a setup has it and whether it is hidden. Identify buzzes it; Forget drops it.
- **Setups:** each running setup with its profile, its pads by role, its virtual pads and identity, and each selection it offers. Start, stop, update, and switch a selection.
- **Profiles:** list, new, duplicate, rename, delete, start. Editing is the preset form below.
- **Settings:** the hiding service's state, the Steam blacklist fallback, starting at login, and the tray's notification switches.
- **About:** TisVlay's, with versions of each part and the diagnostic bundle.

**Profiles are edited through a preset form.** The form covers what the presets take: the mode, which pad has which role, the identity, rumble, starting by itself. A profile the form can't express (a hand-written graph) is shown read-only with what the daemon's `validate` says about it, and a button that opens the file. Arbitrary graphs stay in files; the visual editor stays an idea.

**Keys only, as TisVlay.** The GUI binds no pad. It copies TisVlay's key navigation and couch mode, so a pad drives it wherever the session turns pads into keys (Plasma Bigscreen, Steam's desktop layout). Reason: a running setup may have hidden the pads in the user's hands, and deciding which pad may steer a window while a game is running is a design of its own.

**The live view of a pad is a later step in this plan,** after a spike. It needs pad state to leave the input thread, which nothing does today, and a client must never be able to slow that thread. The first panes are built on what the mirror already has.

**Carried over from TisVlay in the first build:**

- couch mode and the whole key navigation (regions, rows, the Escape chain, value controls entered with Return, the winit key hook), with its tests;
- the contextual help overlay (the "i" toggletips and hover hints);
- translations: `@tr()` and gettext, the `.pot` recipe; the tray stays untranslated, as there;
- the dev tooling: pinned `slint-lsp` and `slint-viewer` in the flake, live preview and the Slint MCP server in `just gui-start`.

**The icons crate and the tray's notification switches come in with the GUI.** `tisvlay-icons` is copied as `ctrlassist-icons` in the step that first draws a theme icon; the tray grows per-category notification switches, served to the GUI as TisVlay's are, in the step that builds the Settings pane. This undoes the rewrite plan's deviation "the tray leaves out TisVlay's notification switches and its icons crate".

**The tray opens the window,** as TisVlay's does: an entry in its menu, activated over D-Bus, so a second click raises the window already open. The desktop file does the same from a launcher.

**The daemon grows a settings rpc.** A small settings file the daemon owns, read and written through an rpc and published like its other state, so the CLI sets the same options the window does. The tray serves its own notification switches, as in TisVlay. Which options there are is still to be listed.

**The preset form is rendered from one served schema.** `ctrlassist-protocol` describes a preset's fields once, the daemon serves it, and the window renders it with TisVlay's schema form. The tray's quick menu and the CLI can move to the same description afterwards.

**The application id is `io.github.ctrlassist.Gui`.** It names the desktop file, the D-Bus activation name and the window. It reads as a GitHub organization named `ctrlassist`, where the repository is expected to move.

**One package, as TisVlay.** `ctrlassist-gui` ships in the same package as the rest, in every format.

**A profile may name a known pad that isn't plugged in.** The form's role pickers list every known pad, the absent ones marked as such; the profile saves, and starting it waits for the pad, as the daemon already does for a source that names an exact pad.

**The settings are two switches:** whether setups hide pads through the hiding service or only grab them, and whether the Steam blacklist fallback may be used. Both are on today with no way to turn them off. Starting at login and a default identity were offered and not taken.

**The tray's quick menu and the CLI move to the served schema in this plan,** so all three describe a preset the same way before the release.

**The daemon gains a `reload` rpc,** with `ctrlassist reload`, that rereads profiles and settings from disk, and the sidebar keeps TisVlay's Reload button and its outcome banner. A hand edit of a profile file isn't picked up today until the daemon restarts. A running setup whose profile changed is brought up to it by the reload: live where the change allows, and by a restart where a virtual pad must be recreated.

**The sidebar's dots are the daemon, the hiding service and the tray.** Without the hiding service pads are only grabbed, which the user should see.

**Pacing:** build through, each decision made on the way recorded under For review; stop only where a rule says to ask or something needs the maintainer's hands or eyes.

**Couch mode is copied whole** (2026-11-21): on by itself under Plasma Bigscreen, a preference (auto, on, off) and a scale in Settings, kept in the window's own `gui.toml` beside the daemon's files, and `CTRLASSIST_COUCH_MODE` to override it.

**The About pane lists four parts** (2026-11-21): the daemon, the hiding service, the tray and the command line, each with its release and health. The tray shows as absent until it is a service, which stays with the step that gives it its notification switches.

**The diagnostic bundle** holds, beyond the releases and the desktop session: the known pads, the running setups, the profiles' text, and facts of the kernel and input (the kernel's release, whether uinput and uhid can be opened, whether the daemon got real-time scheduling, whether Steam runs). Pad addresses are shortened in it, so a pasted report doesn't carry a pad's whole Bluetooth address.

**CtrlAssist's icon is the theme's `input-gaming`,** for the window, the desktop entry and the About header: what the tray uses, and no artwork to ship.

## What is copied, replaced and new

| TisVlay | Here |
|---|---|
| `ui/design.slint`, `ui/theme.slint`, `tests/theme_breeze.rs`, `docs/gui/theme-breeze.md` | Copied. |
| `ui/widgets.slint` | Copied, less `VendorTile`, `ProfileTile`, `ScanLine`; a pad tile is new. |
| `ui/main.slint`, `globals/{nav,couch,help,select,toast,errors,health,overlay,clipboard,icons,daemon,tray,edit_pane}.slint` | Copied; the pane list changes, `Health` loses the session service. |
| `components/about_pane.slint`, `about.rs`, `diagnostic.rs` | Copied; the component rows are the daemon, the hiding service, the tray. |
| `components/settings_pane.slint` | Copied and extended. |
| `components/profiles_pane.slint`, `globals/profiles.slint`, `profiles.rs` | The list, header and dialogs are copied; the field model is replaced by the preset form. |
| `components/schema_form.slint`, `schema_form.rs` | Copied; it renders the preset form. |
| `components/tvs_pane.slint` | Replaced by the Pads and Setups panes, on its list and detail shapes. |
| `components/pairing_wizard.slint`, `wizard.rs`, `power_cycle.rs` | Not copied. |
| `nav.rs`, `couch.rs`, `keys.rs`, `state.rs`, `state_watch.rs`, `toast.rs`, `accent.rs`, `fonts.rs`, `icon_theme.rs`, `file_manager.rs`, `dbus.rs`, `token.rs` | Copied and trimmed; `state_watch` reads CtrlAssist's mirror. |
| `tests/it/{navigation,about_pane,contextual_help,reload_banner,support}.rs` | Copied; navigation is rewritten for the new panes on the same helpers. |
| `packaging/freedesktop/org.tisvlay.Gui.desktop`, the D-Bus activation unit, `packaging/nix/slint-*.nix` | Copied, renamed. |
| `docs/gui/*.md`, `docs/i18n.md`, `docs/concerns/slint-*.md` | Copied, with what was found on TisVlay kept as found there. |

## Open questions

1. **The live view's spike**: what it must show before the step is designed (sampling at a display rate, only while a client asks, from a copy the input thread writes without waiting).
2. **The license bundle,** which TisVlay pins a cargo-about release for because of Slint's license files.

## Steps

Each is a commit or a few, with the gates run and the TisVlay commit named. Order may change as the open questions are answered.

1. `build(gui): the Slint toolchain in the flake` — the pinned Slint crates in the workspace table, `slint-lsp`, `slint-viewer` and the runtime libraries in the dev shell, the license bundle's pin. Verified: the dev shell builds; `slint-viewer --version`.
2. `feat(gui): an empty shell` — both crates, tokens, theme and its Breeze test, widgets, the shell with a sidebar and empty panes, the binary, `just gui-start` with live preview and the MCP server. Verified: the theme test; the window opens and its element tree is read over MCP.
3. `feat(gui): keys and couch mode` — navigation, the Escape chain, the winit hook, with the copied navigation tests over the empty shell. Verified: the tests; by keys in the window.
4. `feat(daemon): a reload rpc` with `ctrlassist reload`, then `feat(gui): the daemon's state in the window` — the mirror, the state watch, health and error slots, the Reload button and its banner. Verified: tests against a daemon on a temp socket, as the tray's are.
5. `feat(gui): the About pane` — brings `ctrlassist-icons`. Verified: the copied About tests.
6. `feat(gui): the Pads pane` — list, detail, identify, forget. Verified: tests with fake pads; identify on a real pad.
7. `feat(gui): the Setups pane` — start, stop, update, selections. Verified: tests; a setup steered on real pads.
8. `feat(protocol): a preset's fields as a served schema`, then `feat(gui): the Profiles pane and the preset form`, then `refactor(tray)` and `refactor(cli)`: the quick menu and the preset flags from the same schema. Verified: tests that a form's profile is what the CLI would write; a profile made and started in the window.
9. `feat(daemon): settings, and an rpc to read and set them` with the CLI's side, `feat(tray): notification switches`, then `feat(gui): the Settings pane`. Verified: tests on both; a switch seen to silence a notification.
10. `feat(gui): contextual help` — the overlay and the help text for each pane. Verified: the copied tests.
11. `feat(gui): translations` — the domain, the `.pot`, the extract recipe and its gate. Verified: the gate; a pseudo-locale run.
12. `feat(gui): activation and packaging` — the desktop file, D-Bus activation, the tray's entry if wanted, the Nix package and module, the other packages' file lists, a VM check that the window opens. Verified: the VM check; the copied activation tests.
13. `docs(book): the window` — a guide chapter with screenshots, and `docs/gui/`.
14. Spike, then `feat: a live view of a pad`.

## For review

- **What the empty shell holds** (build step 2). All of TisVlay's widgets, tokens and theme, and the globals the widgets and the shell reach (`Nav`, `Couch`, `Help`, `Select`, `Toast`, `Errors`, `Health`, `Icons`, `Clipboard`, `Overlay`), came in at once, since `widgets.slint` and `main.slint` import them; the Rust behind navigation, couch mode and help follows in their steps, and until then those globals' callbacks do nothing. The key handling written in Slint (the rail, the pane following focus) already works and is tested. The footer shows the daemon and the tray as absent until the state step. The access banner (TisVlay's socket group) isn't copied: the daemon's socket is the user's own.
- **Keys and couch mode** (build step 3). `keys.rs` is TisVlay's as it stands. `nav.rs` is its history with the TV and profile selections taken out: an entry remembers its pane and where focus was, and each pane adds its selection and its click-throughs as it is built. `couch.rs` is TisVlay's but for the file: TisVlay's helper keeps a hand edit's comments when it rewrites `gui.toml`, and this writes the file plain through the daemon's durable write, which became public for it. The preference's controls come with the Settings pane; until then the file and the environment variable set it. Couch mode was seen on the maintainer's desktop, forced on: maximized, scaled 1.4 over the display's 2.0, the ring on the active sidebar item.
- **Answered, 2026-11-21:** the 2 s wait at exit stays, as TisVlay's; `gui.toml` written plain is fine.
- **One duration came with couch mode:** on exit the window waits up to 2 s for the last preference write to reach the disk, as TisVlay's does, and says so in the log if it didn't. It bounds a write to a disk that has stopped answering; without it a hung filesystem would hold the window's process open. Flagged since a new duration is to be asked about.
- **The reload rpc** (build step 4). `reload` rereads the profiles directory and replies with what changed, what is gone, which changed profiles are wrong, and which running setups it set about updating; the updates go on after the reply, and the setups' published state says how they fare. An update a reload asks for goes down the same path as a client's, with nobody owed a reply: its outcome is logged. A setup that is busy (starting, stopping, or mid-update) when the reload comes is not updated and not retried; `update` or another `reload` does it. A profile whose file is gone leaves its setup running, as deleting it through the daemon does. Settings join the reload when they exist. Not carried over from TisVlay: SIGHUP as another way to ask.
- **The daemon's state in the window** (build step 4). The window's state code is written for CtrlAssist, on TisVlay's rule and not its files: level-triggered, one read of the whole mirror into a `Snapshot` for each burst of events, and the window set from that. TisVlay's `state.rs` and `state_watch.rs` are about TVs nearly throughout. `health`, the reload summary and banner, the toast and `on_ui` are its as they stand. Seen on the maintainer's desktop against the dev daemon: both dots green, and Reload answered with a toast.
- **The tray's dot waits for the tray to be a service.** The sidebar shows the daemon and the hiding service. The tray says nothing on the bus yet that a client could see it by; it gets a liveliness token when it starts serving its notification switches, and its dot comes with that step. The hiding service's dot is what the daemon reports: green when it hides pads, hollow when they are only grabbed.
- **The toast stays up for 2.5 s,** TisVlay's figure: how long a line of text is shown, the reader's time and not a wait for anything. Flagged as a duration all the same.
- **`just gui-start` names the package,** `-p ctrlassist-gui-app`, where TisVlay's doesn't: the window isn't in `default-members` until the packaging step puts it in the package.

## Deviations

- **Build steps 1 and 2 landed as one commit.** The project's rule is that a tool joins in the commit that first gives it something to do, and the Slint tools in the flake have nothing to do before there is a `.slint` file.
