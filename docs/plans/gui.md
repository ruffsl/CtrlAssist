# A settings window, on TisVlay's GUI

## State

- **Status:** decisions settled with the maintainer on 2026-11-21; the build is under way. Build steps 1 to 7 are done: the window opens with its sidebar and five empty panes, is steered by keys, with couch mode, follows the daemon, and its Reload asks the daemon to reread its files.
- **Next action:** build step 8 is part done: presets take `rumble`, and the daemon serves a preset's fields. The Profiles pane and its form are built. Still owed in it: the tray's quick menu from the served fields.
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

**The tray's quick menu moves to the served schema in this plan,** so the tray and the window describe a preset the same way before the release.

**The daemon gains a `reload` rpc,** with `ctrlassist reload`, that rereads profiles and settings from disk, and the sidebar keeps TisVlay's Reload button and its outcome banner. A hand edit of a profile file isn't picked up today until the daemon restarts. A running setup whose profile changed is brought up to it by the reload: live where the change allows, and by a restart where a virtual pad must be recreated.

**The sidebar's dots are the daemon, the hiding service and the tray.** Without the hiding service pads are only grabbed, which the user should see.

**Pacing:** build through, each decision made on the way recorded under For review; stop only where a rule says to ask or something needs the maintainer's hands or eyes.

**Couch mode is copied whole** (2026-11-21): on by itself under Plasma Bigscreen, a preference (auto, on, off) and a scale in Settings, kept in the window's own `gui.toml` beside the daemon's files, and `CTRLASSIST_COUCH_MODE` to override it.

**The About pane lists four parts** (2026-11-21): the daemon, the hiding service, the tray and the command line, each with its release and health. The tray shows as absent until it is a service, which stays with the step that gives it its notification switches.

**The diagnostic bundle** holds, beyond the releases and the desktop session: the known pads, the running setups, the profiles' text, and facts of the kernel and input (the kernel's release, whether uinput and uhid can be opened, whether the daemon got real-time scheduling, whether Steam runs). Pad addresses are shortened in it, so a pasted report doesn't carry a pad's whole Bluetooth address.

**CtrlAssist's icon is the theme's `input-gaming`,** for the window, the desktop entry and the About header: what the tray uses, and no artwork to ship.

**Answered, 2026-11-21, on the About pane and the pads list.** The tray is made visible to clients now, with a presence token and its release, ahead of its notification switches, so its row and its sidebar dot are real. The third-party notices button opens the installed bundle, and in a source build the one committed at `packaging/licensing/THIRD-PARTY-LICENSES.html`, embedded; no cargo-about run in a build script. In the diagnostic bundle a pad's address keeps its vendor half and masks the rest, the same way everywhere, with a short tag to tell pads apart. The Pads pane lists connected pads first, then the remembered ones dimmed.

**The Pads pane's detail** (2026-11-21) shows, beyond the pad's name and its Identify and Forget buttons: its model and how it is connected; which setup has it and in what role, as a link to that setup; whether it is hidden or only grabbed while a setup has it; and its id and address, each with a Copy button. Identify buzzes for 600 ms, the CLI's default. Nicknames for pads are [an idea](../ideas/pad-nicknames.md), not built: the list tells pads of one model apart by a short tag from the address.

**The Setups pane** (2026-11-21) lists only the setups that exist: running, held or failed. A profile is started from the Profiles pane, and an empty Setups pane says so and links there. A setup's detail can stop it, asking first; update it when the kept profile differs from what runs, saying beforehand whether that is seamless or restarts the virtual pads; switch what a toggle or unicast node selects; and jump to each of its pads and to its profile. While any setup is held or failed, the sidebar's Setups item carries a badge, seen from every pane.

Identify from the Pads pane buzzed the maintainer's DualSense, which a running setup held (2026-11-21).

**The preset form** (2026-11-21). The preset is a dropdown with the chosen one's sentence under it, and the fields that don't belong to it disappear. A role's pad is picked from the known pads, by name and tag, the absent ones marked; "any pad of this model" stays something written by hand, and a profile that uses it shows that field read-only. Each change applies at once: there is no draft, every pick is put, and a setup running the profile is updated, as the tray's quick menu does, which restarts its virtual pads where the change needs it (another identity, another count of them). The tray's `quick` is an ordinary profile in the list, with a note that the tray's menu edits it.

**Presets gain a `rumble` key** (2026-11-21): `both`, `primary` or `assist`, on the three presets of two pads, saying who feels what a game rumbles. The tray's `quick` profile was written in the full form only to say that, so a form over presets couldn't have edited it; with the key the tray writes `quick` as a preset, and one description covers the tray, the form and a file. This revisits the rewrite plan's "presets take no overrides" for the one key.

**The served schema is the fields with their rules:** each field's key, label, help, kind (a choice, a pad, a number, a switch), its choices with their labels and descriptions, its default, and which presets it belongs to. A client renders it and writes the profile's TOML; the daemon still checks every profile it is given. A new preset or field then needs no change in a client.

**The CLI has no preset flags to move,** which an earlier line of this plan assumed: profiles go in as files. Making one from flags is [an idea](../ideas/profile-new.md). The schema's users in this plan are the window and the tray.

**The Profiles pane, further** (2026-11-21). New asks a name and opens the form on a draft the window holds; the first moment its picks make a profile the daemon keeps it is put, and from then on each change applies at once; a draft left unfinished is dropped on leaving it, after a confirm. A change to a profile whose setup runs is applied to the setup at once and never asks, restart or not, as the tray's quick menu does. A profile in the full form shows its text read-only, with what the daemon finds wrong with it, its Start, Duplicate and Delete, and a button that shows its file. The tray's quick menu keeps to the presets of two pads, its labels and choices from the served fields.

**Answered, 2026-11-21, on the tray and settings.** The tray's quick menu shows nothing of its own until the daemon has served the fields: it says it is waiting for the daemon. Four kinds of notification get a switch each: a setup started or stopped; a setup held or failed; a pad connected or left; something the tray asked for was refused. For the window to reach the tray's switches the maintainer chose to make the daemon a zenoh router, so that what one client serves reaches another, and to copy TisVlay's tray service as it is, over leaving the switches with the daemon or relaying. TisVlay's architecture notes give the shape: a router hub, and the serving client on a client-mode link that is opened fail-fast and retried, since a connect-only peer's declarations don't fan out through any hub. That changes the control plane's foundation, so it starts with a spike.

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

## Spike: serving from one client to another

**Question.** The maintainer chose to make the daemon a zenoh router so that the tray can serve its own notification switches to the window, as TisVlay's tray does on its second socket. Does that work on CtrlAssist's zenoh, and what does it take?

**Method.** [`spikes/router`](../../spikes/router): a hub on a Unix socket, a window on a connect-only peer link as every client has today, and a tray serving a liveliness token, a queryable and a cached publication. What the window saw of the three, zenoh 1.10.2, the same on three runs:

| Hub | Tray's link | Token | Reply | Cached state |
|---|---|---|---|---|
| peer (as now) | connect-only peer (as now) | not seen | not seen | not seen |
| peer (as now) | client mode | seen | seen | seen |
| router | connect-only peer | not seen | not seen | not seen |
| router | client mode | seen | seen | seen |

The hub itself saw all three in every case.

**Finding.**

- **It is the tray's link that decides, not the hub's mode.** A leaf on a client-mode link has what it serves relayed to the other leaves, by a peer hub as by a router. A leaf on a connect-only peer link doesn't, under either. So the daemon need not become a router: it can stay the peer it is, and nothing about the existing clients or the daemon's session changes.
- **This differs from TisVlay's note,** which says a serving leaf needs a client-mode link *into a router*. That was written for zenoh 1.9, and TisVlay's hub for the tray is a router, so the peer-hub case may never have been tried there. Here it was tried and it relays.
- **A client-mode link has no hub to wait for.** Opening one with no hub fails at once, where a connect-only peer opens and dials forever. So the tray, which starts before or without the daemon, needs a second session for serving, opened when the daemon appears (its liveliness token on the tray's first session is the event; no retry timer is needed).
- **The link survived the hub restarting.** With the hub closed and opened again on the same socket, the tray's client session, left alone, was seen by the window again with all it served, within the 2.5 s the spike waited. TisVlay reopens its link in a loop; here that wasn't needed in the one restart tried. How it behaves across a longer absence wasn't measured.

**What it leaves open.**

- Whether to still make the daemon a router. The spike finds no need. A router also changes how timestamping and routing are configured for every existing client, which is the risk the maintainer's choice carried, and it buys nothing the client link doesn't.
- The tray's serving session across a daemon that is gone for longer, and whether one opened on the daemon's token ever needs opening again.
- The window's side: the SDK's mirror and release check for a second service, which TisVlay has as `TrayClient` and CtrlAssist's SDK doesn't.

**Proposed.** Keep the daemon a peer. Give the tray a serving session on a client-mode link, opened when the daemon appears, and copy TisVlay's tray settings service and its `TrayClient` onto that. The tray's presence, now reported by the daemon, could then come from the tray's own token, and the daemon's relay of it be removed.

## Steps

Each is a commit or a few, with the gates run and the TisVlay commit named. Order may change as the open questions are answered.

1. `build(gui): the Slint toolchain in the flake` — the pinned Slint crates in the workspace table, `slint-lsp`, `slint-viewer` and the runtime libraries in the dev shell, the license bundle's pin. Verified: the dev shell builds; `slint-viewer --version`.
2. `feat(gui): an empty shell` — both crates, tokens, theme and its Breeze test, widgets, the shell with a sidebar and empty panes, the binary, `just gui-start` with live preview and the MCP server. Verified: the theme test; the window opens and its element tree is read over MCP.
3. `feat(gui): keys and couch mode` — navigation, the Escape chain, the winit hook, with the copied navigation tests over the empty shell. Verified: the tests; by keys in the window.
4. `feat(daemon): a reload rpc` with `ctrlassist reload`, then `feat(gui): the daemon's state in the window` — the mirror, the state watch, health and error slots, the Reload button and its banner. Verified: tests against a daemon on a temp socket, as the tray's are.
5. `feat(gui): the About pane` — brings `ctrlassist-icons`. Verified: the copied About tests.
6. `feat(gui): the Pads pane` — list, detail, identify, forget. Verified: tests with fake pads; identify on a real pad.
7. `feat(gui): the Setups pane` — start, stop, update, selections. Verified: tests; a setup steered on real pads.
8. `feat(protocol): presets take a rumble key`, `feat(protocol): a preset's fields as a served schema`, then `feat(gui): the Profiles pane and the preset form`, then `refactor(tray)`: the quick menu from the same schema. Verified: tests that a form's profile is what the CLI would write; a profile made and started in the window.
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
- **The tray is visible through the daemon** (2026-11-21, asked for ahead of its notification switches). What one client of the daemon's socket declares doesn't reach another, which TisVlay found and worked around with a second socket; only the daemon sees it. So the tray declares a liveliness token that carries its release, the daemon follows those tokens and says in its own state that a tray is there, and the window reads that. A tray is therefore only ever seen through a running daemon: with the daemon down its dot is hollow and its row says unknown. Tested through the SDK against a live daemon, across the daemon restarting under a tray that stays. Its notification switches will need more than this: an rpc from one client to another has the same problem, so they will go through the daemon too, or over the session bus. The hiding service's dot is what the daemon reports: green when it hides pads, hollow when they are only grabbed.
- **The toast stays up for 2.5 s,** TisVlay's figure: how long a line of text is shown, the reader's time and not a wait for anything. Flagged as a duration all the same.
- **The About pane, so far** (build step 5). `ctrlassist-icons` is TisVlay's crate as it stands, and the sidebar and the action glyphs now come from the desktop's theme. The pane's Slint is TisVlay's with its text changed; the Diagnostics and third-party notices buttons are taken out until what they do is built, so no button is dead. `about.rs` is written for CtrlAssist's parts on TisVlay's model: the rows are made from what the window already shows, so they can't disagree with the sidebar's dots. There are five rows, not the four asked for: the window itself has one, as TisVlay's has, since its `gui.toml` is a file a user may want shown. The daemon's published state gained `config`, the directory it keeps the user's files in, so the Config button shows the right folder for a daemon started with another one. Strings are plain until the translations step wraps them.
- **The third-party notices, and a wrong premise in the question asked** (build step 5). The question offered "the committed one" as the bundle a source build embeds, and that was chosen; but the bundle isn't committed. It is written by `just licenses-html`, which the commit hooks and every package build run, and git ignores it. What is built keeps what was asked for, no cargo-about run in a build script: the window's `build.rs` copies the bundle from where that recipe writes it, and a tree where it was never written gets a page saying how to write it. An installed copy is opened in preference, as in TisVlay.
- **The diagnostic bundle** (build step 5). JSON, as TisVlay's is, with a schema version: the window's release and build, each part as its About row has it, the daemon's own state, facts of the machine (kernel, distribution, desktop and session type, whether uinput and uhid can be opened, whether Steam runs), and the pads, setups and profiles as the daemon publishes them, whole. Addresses are masked in the finished text, so nothing that carries one slips through by being a field nobody thought of: `0c:27:56:xx:xx:xx#1`, the number telling two of one maker's apart. A pad's name is free text and goes in as it is. Left out against TisVlay's: the C library's version, which takes `unsafe` to ask for, and the compiler and commit the window was built from. The distribution is read from `os-release` directly; TisVlay's crate for it mangles NixOS's version. The Copy button was not pressed on the maintainer's desktop, since it replaces what the clipboard holds.
- **The Pads pane** (build step 6). Its Slint is written for it on the shapes of TisVlay's TVs pane: a list that answers column requests and steps its own rows, a detail whose rows carry row ids, a confirm before Forget. Its model is a global, `Pads`, as TisVlay's profiles are, where its TVs are properties of the window. What differs from what was asked: the model's name from the controller database isn't shown, only the pad's own name and its vendor and product numbers, since the database is the daemon's and names nothing a client can read; and the daemon's pad state gained `bus`, the bus a pad is connected on, for the Connection row. The setup link goes to the Setups pane, and to the setup itself once that pane exists. The actions sit under the pad's name, not beside it: a name as long as a DualSense's pushed Forget off a window at its smallest. Seen on the maintainer's desktop with the dev daemon: both pads of `quick` and a remembered one, and Identify pressed without a refusal; whether the pad buzzed is theirs to say.
- **The Setups pane** (build step 7). On the shapes of the Pads pane. For the Update button the daemon now says when a setup's profile, as kept, is no longer the one it runs (`stale` on the setup); what an update would cost is asked when the button is pressed and shown in the dialog that confirms it, not worked out for every stale setup beforehand. Stop asks first, as asked for. The history behind Alt+Left now remembers the pad and the setup selected, and the links between the two panes go through it, so back returns to what was being looked at; the link to a setup's profile goes to the Profiles pane, and to the profile itself once that pane exists. Each row has a detail of its own and one shows, as in TisVlay; the hidden ones' controls now go by other ids, after a test found that putting focus back on a control by its id could land on a hidden copy and leave the keys dead. A pad's name longer than its row is cut at the row's edge. Seen on the maintainer's desktop with the dev daemon: `quick` running, its two pads there, its virtual DualSense. Stop, Update and a selection were not pressed there: a game was using the setup.
- **Presets take `rumble`, and the tray writes `quick` as a preset** (build step 8). The key expands to the mux's `feedback`, as the full form would say it, and is refused on the presets of one pad and outside a preset. The tray reads any preset of two pads as its choices and no longer reads the full form it used to write: a `quick` profile kept from before this commit shows in the menu as nothing chosen until a pick is made, which writes it anew. Nothing released wrote the old form, so nothing reads it.
- **The served schema** (build step 8). `ctrlassist-protocol::form` describes a preset's eight fields once (the preset, primary, assist, pad, outputs, identity, rumble, start by itself), and the daemon serves them on `profile/schema`. The same module turns a form's values into a profile's TOML and reads one back, so the window and the tray share that too and neither writes TOML of its own; a profile a form can't show, the full form among them, reads back as nothing. It is CtrlAssist's own shape, not TisVlay's pairing schema, which is about text fields with formats and autofill: here every field is a choice, a pad, a number or a switch. A test writes every preset with every field at each of its values and has the profile checked as the daemon would.
- **The Profiles pane and its form** (build step 8). The list and the name dialogs are the shapes of TisVlay's profile list; the form is written for the served fields, one row per field by its kind, where TisVlay's profile editor is about its own field model of overrides and resets. Each change applies at once, as asked: a field set is a put, and a setup running the profile is updated with no question. A change that doesn't make a profile can't be put, so the window holds it: a new profile until its pads are picked, and a kept one whose preset was swapped for one of other pads. The form shows what is held, the row says unfinished and why in the daemon's own words (the same check, run in the window), and the next change that completes it is put. Leaving such a profile asks whether to drop the change or go back to it. What is held goes once the kept profile says the same.
- **Rows are updated in place** (found while trying the form on the maintainer's desktop). Every change the daemon reported replaced a pane's rows with a new model, so Slint rebuilt every detail: the form jumped back to its top and the control with keyboard focus was gone after each pick, and the next key went nowhere. The rows' data is now replaced in the model Slint already has wherever the rows are the same things, down to a form's fields and a setup's pads and choices, in all three panes.
- **Not in the Profiles pane:** a pad can't be picked as "any of this model", as decided, and such a field is shown and not changed; rename is refused while the profile's setup runs, since a setup goes by its profile's name. An earlier note here said a dropdown's keyboard cursor starts at its first choice; that was wrong, read off one line of the select and a key sequence sent blind that wasn't landing where it was thought to. A test opens a dropdown twice by keys and finds the cursor on the current choice both times, with TisVlay's select unchanged.
- **`just gui-start` names the package,** `-p ctrlassist-gui-app`, where TisVlay's doesn't: the window isn't in `default-members` until the packaging step puts it in the package.

## Deviations

- **Build steps 1 and 2 landed as one commit.** The project's rule is that a tool joins in the commit that first gives it something to do, and the Slint tools in the flake have nothing to do before there is a `.slint` file.
