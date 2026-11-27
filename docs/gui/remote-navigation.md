# Remote and keyboard navigation (GUI)

How `ctrlassist-gui` is driven by keys alone: a gamepad or a TV remote under a 10-foot shell such as Plasma Bigscreen, or a keyboard on a desktop. This is the model behind it and where each part lives. It is TisVlay's, copied whole with the window (TisVlay `docs/gui/remote-navigation.md`); what was measured or traced there is kept as TisVlay found it, and the parts that were about TVs, its pairing wizard and its config editor are left out.

## The contract

**The window binds no gamepad; it reads keys.** A running setup may have hidden the pads in the user's hands, and deciding which pad may steer a window while a game runs is a design of its own (the GUI plan's "Keys only"). Bigscreen's input handler turns gamepads (SDL3) and CEC remotes (libcec) into evdev key presses through the xdg RemoteDesktop portal, so the window sees an ordinary Wayland keyboard. A key-level contract works under any session that maps a pad or a remote to keys, and with a keyboard.

**Invariant: every function is reachable with arrows, Return and Escape alone.** Tab and Menu are accelerators, since CEC remotes have neither. Space, Alt+arrows and the mouse's back and forward buttons keep working. The vocabulary, as TisVlay checked it: the gamepad column on a KDE neon VM with Plasma Bigscreen and a DualShock 4, the remote columns traced through Bigscreen's input handler source, the kernel's `rc-cec` keymap, xkb and winit:

| Key | Gamepad | CEC remote, through Bigscreen | Remote keys through the kernel's `rc-cec` keymap |
|---|---|---|---|
| Arrows | D-pad, left stick | Arrows | Arrows |
| Return | Cross (A) | Select | Select |
| Escape | Circle (B) | Back/Return, Exit | Exit (remapped from `XF86Close`) |
| Tab, Shift+Tab | R1, L1 | — | — |
| Menu | Square (X), remapped from `XF86MenuKB` | — | Info (remapped from `XF86Info`) |
| Escape | Share, L2, remapped from `XF86Back` | — | — |

- Escape is Back on both: the gamepad's Circle sends Cancel and Escape together, and the CEC Back key sends Escape.
- L1 arrives as Shift+Tab, never as Backtab. Slint turns Shift+Tab into Backtab itself, so a handler that wants "previous" checks both.
- Slint's winit backend drops a key that has neither a Slint name nor text. The gamepad's Square sends KEY_MENU, which xkb maps to `XF86MenuKB` (the keyboard's Menu key is KEY_COMPOSE and arrives named), and Share and L2 send KEY_BACK, `XF86Back`, which winit names `BrowserBack` but Slint has no name for. Through the kernel's CEC keymap, Exit is KEY_EXIT, `XF86Close`, which winit names `Close` and Slint doesn't, and Info is KEY_INFO, `XF86Info`, which winit leaves unidentified. The winit hook remaps all four ([below](#the-winit-hook)). Forward, Games, Cancel and Home are dropped and stay dropped.
- The on-screen keyboard follows text focus from the remote's own input path. Bigscreen sets `KWIN_IM_SHOW_ALWAYS=1`, and its keyboard rises as soon as keyboard focus reaches a text field from the gamepad; a hardware keyboard, or a virtual one injected into a VM, doesn't raise it, since KWin shows the keyboard by the last input device.

## One focusable base, one ring

`Focusable` (`ui/widgets.slint`) is a Rectangle-rooted component with an inner `FocusScope` behind its `TouchArea`. The scope has `focus-on-click: false`, so the first click isn't spent on taking focus; the click handler focuses explicitly, so a pointer user's next keys continue from the control they clicked. The base owns `accessible-role`, `accessible-enabled`, `accessible-action-default` and Return/Space activation, all calling one `activated` callback. The std Button, Switch and CheckBox are replaced by themed widgets on it, as `ThemedSelect` is; `LineEditField` is a bare `TextInput` in the shared control box (a std `LineEdit` leaves the Tab order only through `enabled: false`, which dims it, and a read-only one stays a stop). The accessible actions double as the test surface: the `accessibility` feature is off in production.

The ring (`FocusRing`) is an accent outline, `Theme.focus-ring`, `Design.ring-width` wide at `Design.ring-offset` outside the control with the control's corner radius; `ring-inside` draws it inside a row that fills a clipped column. It shows only while `Nav.focus-visible` is set: the shell's capture handler sets it on a navigation key (Tab, the arrows, Return, Space) and a pointer press on a control clears it, so a mouse user never sees rings trail their clicks. The ring stays on a focused control that turns disabled. A landing made without a key (a history step) places focus but doesn't show the ring; the next navigation key does.

What a focusable tells the shell: `column` (0 the sidebar, 1 a list, 2 the detail or a single-column pane), `tab-stop` (whether Tab visits it), `nav-row` (the detail row it sits in), `reveal` (whether a pane scrolls to keep it in view), and the `focus-in` callback, which runs however focus arrived.

## Regions

Three columns, sidebar, list and detail, and inside the detail, rows. Up and Down step inside the region focus is in; Left and Right go to the focused control first, then walk the row, then jump columns.

- **The sidebar** is a rail: the four items (Pads, Profiles, Settings, About), then a row for each running profile, then Reload, stepped by the shell itself (`rail-step` in `ui/main.slint`), stopping at both ends. The pane follows focus along the four items, and Return or Right enter the pane: its list's selected row, or its first control. A click stays on the item. A running profile's row belongs to no pane, so the pane stays as it was with focus on one, which is About's by the way there; Return on a row opens its profile. The rows are made from a model, so the shell asks one to take focus by its profile's name, and a row made again while its profile's had focus takes it back, as the profiles that run come and go.
- **A list** (the pads, connected over remembered; the profiles, running over the rest, with the "+") steps its own stops. The shell asks through `Nav.request-list-step`; the focused row or "+" answers by focusing its neighbor and marks the request served, and the shell's 1 ms fallback steps the Tab order when none does. Focusing a row selects it, however focus got there (a key, a click, a landing the shell placed), so a list never shows an empty detail. Return or Right enter the detail's first stop: a profile's Start or Stop; a pad's Identify, or Forget for a pad that isn't connected.
- **Roving Tab stops.** Only the active sidebar item, one of the running profiles' rows (the one last focused, or the last there is) and the selected row of a list (the first, with nothing selected) are Tab stops (`Focusable.tab-stop`, Slint's `focus-on-tab-navigation`), ARIA's roving tabindex. A desktop Tab goes active item, a running profile's row if there is one, Reload, "+", selected row, detail, without switching or selecting anything on the way. The detail's stops all stay Tab stops.
- **The detail steps by rows.** A row component (`FormRow`, `LabeledRow`, the profile field rows, the detail's header and status rows) takes an id from `Nav.next-row-id()` as it's created and hands it to its stops through `nav-row`; every stop reports its column and row through `Nav.stop-focused` as it takes focus, with its `accessible-id`: `Focusable`, the select and the text field synchronously from `focus-gained`, the spin box from the deferred `changed has-focus`, which the walk flushes between hops. Up and Down land on the previous or next row's first stop, its control; Right walks out to the row's trailing icons (the "i", reset) and Left walks back. Every icon stays a stop: nothing hides behind a contextual key. The walks run in Rust (`keys.rs`): `row_step` synthesizes Tabs until the row changes (a Backtab then walks on to that row's first stop), flushing the deferred change handlers between hops, and undoes a walk that changed the column instead, so the detail stops at its ends; `stop_step` takes one Tab and undoes it when the row changed, returning false so the shell jumps columns. A stop in no row (0: a dialog's button, a pane that isn't a form) steps as a row of its own.
- **Value controls, in couch mode.** A focused spin box is idle until Return enters it (`Nav.value-idle`): idle, Left and Right walk its row, and the shell takes them in its capture phase before the control would; entered, they adjust the value, and Return or Escape leaves; the caret says it is entered. On a desktop Left and Right go to the control first, as everywhere. A spin box commits on the way out only what was typed into it: a row walk passes through it too, and must not write the value it shows. Change handlers run newest first, so when focus moves straight from one value control to the next the losing control's handler runs last: `stop-focused` clears `Nav.value-idle` for every new stop, a value control sets it after reporting, and a control clears it on focus loss only while it's still the last to have reported.
- **Column jumps.** Left from a detail goes to its selected list row, Left from a list or a single-column pane (Settings, About) to the active sidebar item, Right from the sidebar into the pane. The requests go through `Nav.request-column`: the pane that exists answers by observing `column-tick`, and every answerer marks the request served (`column-served`), because a list created by the same change that made the request checks that marker in its `init` instead, and an unmarked old request would pull focus into the next list created. A list with nothing to focus sends the request back to the sidebar.

## Keys, in the shell

`nav-scope` (`ui/main.slint`) is the window's `forward-focus` and the home of key handling, not a Tab stop (`focus-on-tab-navigation: false`: a stop there is invisible and inert, and Shift+Tab from the first control would land on it instead of wrapping). It keeps `focus-on-click`, which is how a click on empty background dismisses a pinned help bubble.

Its capture phase sees a key before the focused control does: it counts the key (`Nav.key-seen`), reveals the ring on a navigation key, restores the last column on an arrow while the scope itself has focus (a fresh window, a click on empty background) rather than walking from stale state, takes Up and Down for `step-focus` unless a select's list is open or Alt or Ctrl is held (so a text field's caret moves don't keep them), takes Left and Right for `horizontal-step` while a value control is idle, and takes Menu for `Help.menu-toggle(Nav.focus-row)`. Its bubble phase gets the keys no child took: Alt+Left and Alt+Right for history, a bare Left or Right for `horizontal-step` (along the row, then across the columns), Tab and Shift+Tab for couch mode's pane cycle unless `Nav.synthesizing` marks a Tab Rust made for an arrow, and Escape for the chain below.

The bubble phase is what makes the surfaces that own a key win without a modal flag: a dialog cycling its buttons, an open select, an editing field, an entered spin box and a pinned "i" take Tab or Escape before `nav-scope` sees them.

## Escape

One level out per press, in `escape()`, after the surfaces that take it themselves (a select's list, a dialog, an editing field, an entered spin box, a pinned "i"):

1. A visible help bubble closes, however it opened.
2. A detail goes to its selected list row, from any of its stops (Left is the way back to a row's control).
3. A list, or a single-column pane, goes to the active sidebar item.
4. On the sidebar: history back if the page was reached through a click-through (a pad's profile, a profile's pad; `Nav.click-through-back`, kept by Rust), else couch mode quits the process (`Nav.close()`, `slint::quit_event_loop`), as Bigscreen's apps end on Back. A desktop does nothing.

Going up the hierarchy before back in history is how Bigscreen's apps, Android TV and Kodi behave.

## Landings and anchors

Every transition names where focus lands:

| Transition | Focus lands on |
|---|---|
| Dialog closes | Its opener; the selected list row or active sidebar item if the opener is gone (a forgotten pad, a deleted profile) |
| Field reset | That row's control |
| Return or Right on a sidebar item | The pane's selected row, or its first stop |
| Return or Right on a list row | The detail's first stop |
| A click-through (a pad's profile, a profile's pad) | The new pane's detail |
| History back | The stop that triggered the forward move, else that page's column; a page left through the sidebar keeps no stop, since the item focused by then is the destination's and would navigate forward again |
| A detail gone under focus (its pad forgotten, its profile deleted) | Its list, through `has-selection` change handlers |

Slint can't hold an element reference, so a landing is an **anchor**: every stop's focus-in bumps `Nav.focus-seq`, and a focusable keeps the count as its anchor, with its `accessible-id`. A dialog records anchor, id and column at open and asks for them back on close through `Nav.request-anchor(seq, id, column)`. The focusable holding the count (a living instance, a dialog's opener) or the id (one re-created with its pane, which is what history back needs: the count named an instance the pane switch destroyed) focuses itself and marks the request served, and the shell's 1 ms fallback requests the column when none does. A focusable also takes its anchor on activation, which may open a dialog with no focus change; `focus-gained` being synchronous, a control created and focused in one pass reports too, and its `init` runs the `focus-in` callback the deferred handler would miss. A request is answered only by a stop that can take focus (enabled and focusable, `Nav.anchor-matches` deciding the match). A detail exists for every row, hidden when not selected, so the hidden ones' ids carry a `hidden-` prefix (`id-prefix`) and an anchor found by id names the one shown. History entries (`nav.rs`) store the anchor, the id, the column and whether the page was a click-through; a return runs a tick after the pane switch (a zero `slint::Timer`) so the new stops can answer.

## Lost focus recovers

Slint clears focus when the focused item is hidden or destroyed, and with nothing focused it runs no key handler at all, only its built-in Tab: a remote's keys would go dead after a pane switch or a refresh that destroyed the focused control. `has-focus` is set only on the focused scope, never on an ancestor, so the shell can't see "nothing has focus" directly. Instead `nav-scope`'s capture handler counts every key it sees, and the key router (the winit hook in production, the test helper in tests) notes the count before a navigation key and checks it a tick after the dispatch (`keys::key_seen`, `keys::recover_if_unhandled`): a key that didn't move it arrived with no focus, and `restore-focus` puts focus back on the last column, where the pane picks the stop.

## Scrolling keeps a margin

Slint's own scroll-into-view brings the focused item alone to the viewport's edge, which cut off a row's label and its group's title as focus walked down a detail. Every stop reports its window rectangle as it takes focus (`Nav.reveal`, through `Nav.stop-focused-at`), and the pane's `HelpScrollView` nudges its viewport so the rectangle keeps `Design.scroll-margin` above and below, enough for a neighbor row or a group's title band. A dialog's buttons float over the pane and opt out (`Focusable.reveal: false`); lists keep Slint's reveal, a row being the whole stop.

## Help from the keyboard

Each "i" is a `Focusable` stop of its row, the last one: Return or Space toggles its pin. Menu toggles the focused row's help without moving focus. Help pinned from the keyboard follows the row it was pinned from (`pinned-for-row`): it closes when focus leaves that row, on Escape, or when another pins. An icon-only button shows its tip when focus lingers on it, through the hover path, in both modes.

## Dialogs

`ConfirmDialog` focuses Cancel, the safe choice; `NameInputDialog` focuses its field, editing at once, so couch mode's keyboard rises. Both cancel on Escape, cycle Tab and Shift+Tab/Backtab among their own enabled controls with explicit `focus()` calls (skipping a disabled confirm), and land per the table above on close; Return in the name field confirms. The two components cover every dialog in the window, since Slint's traversal can't be scoped to a subtree.

## Couch mode

The layout for a screen across the room (`couch.rs`): a pure function of the preference, the `CTRLASSIST_COUCH_MODE=auto|on|off` override and the session's signals, where Auto turns it on when `PLASMA_PLATFORM` lists `mediacenter`, the key Bigscreen's own apps pick their TV layouts by. The preference (mode and scale factor) is `gui.toml` in the user's config directory, beside the daemon's files. An environment variable rather than a flag, because the window is D-Bus-activated with no arguments and a second launch hands off to the running window. The Settings pane shows the scale as a percentage (`ThemedSpinBox` is an integer control) and the file stores a factor. A file that doesn't parse when the window opens is set aside as `gui.toml.edited`. Saves go through one writer task, in order, the latest value last.

What it changes:

- The window opens maximized, and the UI scales by the preference's factor on top of the compositor's scale: the winit hook multiplies each `ScaleFactorChanged`, and puts the factor on at the window's first `Resized`, since Slint creates the winit window when the event loop starts, after `show()`, and sets the compositor's raw scale past the hook then; the Settings path reads the live compositor scale from the window. `SLINT_SCALE_FACTOR` isn't used: it replaces the compositor's scale, and setting it in-process would mean `set_var` after the runtime's threads start. The window's minimum size is relaxed, since a scaled maximized window can be smaller in logical pixels.
- Focus starts on the active sidebar item at launch, ring showing, so a remote user sees where they are before pressing anything, and on re-activation when nothing has focus (an open dialog or an editing field keeps it).
- Tab and Shift+Tab cycle the panes Pads, Profiles, Settings, About, as console menus do with their shoulder buttons.
- Text fields stop at a ring first and start editing on Return (which brings up the on-screen keyboard); Return or Escape stops editing and keeps focus on the field. Spin boxes are entered the same way.
- Copy buttons leave the focus order: a clipboard is little use on a couch session.
- Escape on the sidebar quits.

A uniform relative scale covers the literal px values a token-only large mode would miss, and still honors the session's own display scale. The default scale came, in TisVlay, from Bigscreen's body font over the GUI's, not from a real TV; the preference lets it be tuned from use.

## The winit hook

`on_winit_window_event` in `lib.rs`, on the `unstable-winit-030` surface ([slint-unstable-winit-030.md](../concerns/slint-unstable-winit-030.md)), runs before Slint sees an event:

- `MouseInput` Back and Forward drive the history.
- `ScaleFactorChanged` is multiplied by couch mode's factor and the original prevented; `Resized` puts the factor on once the window exists, idempotently.
- `KeyboardInput` for a key the backend would drop goes through `keys::remap_dropped_key` (`BrowserBack` and `Close` to Escape; the unidentified Xkb keysyms `0x1008ff65`, `XF86MenuKB`, and `0x10081166`, `XF86Info`, to Menu); the hook dispatches the Slint key instead and prevents the original.
- `KeyboardInput` for a navigation key wraps the dispatch in the focus-loss watch (`watch_focus_after`).

The hook can't do the arrow routing itself: running before Slint, it can't tell a key a control used from one nobody used, which is why the arrows are routed in `nav-scope` and walked through `Nav`'s callbacks.

## Where the code is

- `crates/ctrlassist-gui/ui/globals/nav.slint`: the `Nav` global, the whole shell-to-widget surface (focus visibility, the column and its requests, the key count, list steps, anchors, rows, the idle value flag, reveal rectangles, the click-through flag and `close`).
- `crates/ctrlassist-gui/ui/widgets.slint`: `Focusable`, `FocusRing`, `ListRow` (the base under the pad and profile rows: the list column, the inside ring, `clicked` on Return, Space and Right, the list's focus tick and `step`, the selected row's colors), `LineEditField`, `ThemedSpinBox`, `ConfirmDialog`, `NameInputDialog`. A stop that both reports its place and reveals its rectangle calls `Nav.stop-focused-at`.
- `crates/ctrlassist-gui/ui/main.slint`: `nav-scope`, the rail (its order defined once in `pane-index` and `pane-at`; the four items are named only in `focus-pane-item` and `focused-pane-index`), the column requests and their fallbacks, `escape()`, `cycle-pane`, `restore-focus`.
- `crates/ctrlassist-gui/ui/components/*.slint`: the rows and their ids, the lists' step handlers, the panes' landing handlers.
- `crates/ctrlassist-gui-app/src/keys.rs`: the key router (synthesized Tabs, the row walks, the remap, the focus-loss check); `nav.rs`: the history and its anchors; `couch.rs`: couch mode; `lib.rs`: the hook, the scale, launch focus.
- `crates/ctrlassist-gui-app/tests/it/navigation.rs` and the panes' suites: how keys are driven and focus observed is in [docs/testing.md](../testing.md#driving-keys).

## Lessons, from TisVlay

What TisVlay's VM checks and tests taught about this shared code, kept so the next change doesn't re-learn it.

- **Tab couldn't reach a text field below the fold.** Slint 1.18's traversal skipped a stop under a clipping ancestor whose clip rect was empty while scrolled out of view, and `LineEdit` clips its `TextInput`, so `LineEditField` took the Tab in a scope outside the clip and forwarded focus programmatically. Slint 1.19 reaches the clipped input itself; the scope now serves couch mode's edit-on-Return alone, and the focusable base keeps its scope outside any clip.
- **Return-to-select felt wrong; selection follows focus.** A pane switch and a selection are cheap and local, which is when ARIA's tabs pattern prescribes automatic activation, and master-detail lists preview the focused row on desktops and TVs alike. The linear Tab order would then have switched panes on every Tab through the rail, so the rail and the lists became regions with one roving stop.
- **Stepping every widget of a row felt clunky; the detail navigates by rows.** The tagged Tab order was chosen because the Tab order already matches each row's reading order and Slint offers no other "next".
- **An anchor by count landed on the wrong stop.** The count named an instance the pane switch had destroyed. Anchors now carry the `accessible-id` too, and the request is made a tick after the pane switch.
- **A dialog closed onto the list row instead of its opener.** The opener was created and focused in one pass (a detail focusing its first stop from `init`) and never saw a `changed has-focus`; anchors are taken on activation as well, and the report moved to the synchronous `focus-gained`. `Focusable`'s `init` runs `focus-in` for it until a Slint release delivers that change (slint-ui/slint#14894, fixed by slint-ui/slint#14927 after 1.19 was branched).
- **Change handlers run newest first.** A control losing focus runs its handler after the one gaining it, which left two adjacent value controls live in couch mode.
- **The couch scale compounded on every resize, and took focus with it.** The window the winit hook is handed is Slint's, whose `scale_factor()` is the scale already applied, so reading the compositor's scale from it multiplied the factor in again each time. The compositor's scale comes from `with_winit_window` now.
- **Deleting under focus left focus dead.** The detail that goes away hands focus to its list through a `has-selection` change handler, so the next key isn't spent on recovery.
- **Couch mode's pane cycle stole Tab from dialogs** while it lived in the capture phase; it moved to the bubble phase, where a dialog cycles its own buttons first. The same placement made a modal flag for Escape unnecessary.
- **The Menu key opened a text field's context menu on its release** after the press was captured; Slint 1.19 opens the menu on the press only (slint-ui/slint#14892).
- **A focus ring was clipped by a `Flickable`:** a box that scrolls overhangs by the ring's room.
