# Ideas

Deferred work and designs set aside, one file each. An entry says what the idea is, why it was deferred, and what would bring it back.


## Clients

- [`gui.md`](gui.md) — a settings window.
- [`tui.md`](tui.md) — a terminal interface.
- [`cookbook.md`](cookbook.md) — 0.4.0's recipes, untried on the rewrite, as a page of the user guide.
- [`cli-watch.md`](cli-watch.md) — a verb that stays running and prints each change.
- [`graph-editor.md`](graph-editor.md) — editing a graph visually.
- [`switch-chords.md`](switch-chords.md) — a button chord as the switch.

## Pads

- [`non-gamepad-pads.md`](non-gamepad-pads.md) — wheels, flight sticks and levers.
- [`pad-nicknames.md`](pad-nicknames.md) — a name the user gives a pad.
- [`bundled-tile-glyphs.md`](bundled-tile-glyphs.md) — the profile tiles' glyphs carried with the window, for themes without Breeze's.
- [`profile-new.md`](profile-new.md) — a preset profile from command-line flags.
- [`choice-descriptions.md`](choice-descriptions.md) — a choice's own sentence, where it is chosen.
- [`restart-warning.md`](restart-warning.md) — saying that a game may need restarting after its virtual pad is replaced.
- [`relative-controls.md`](relative-controls.md) — gyro, touchpad, mouse and keyboard.
- [`gyro-bias-at-rest.md`](gyro-bias-at-rest.md) — measuring each pad's gyroscope bias while it lies still, in place of the bias the pad stores.
- [`show-who-has-the-motion.md`](show-who-has-the-motion.md) — the tray and the window saying whose motion sensors the game has, as they say who has control.
- [`touchpad-movement-blended.md`](touchpad-movement-blended.md) — in average, one finger that moves by the mean of both pads' fingers' movement.
- [`larger-pad-state.md`](larger-pad-state.md) — more than 64 buttons or 16 axes.
- [`split-preset.md`](split-preset.md) — a preset for splitting one pad into several.
- [`steam-controller-extras-to-games.md`](steam-controller-extras-to-games.md) — a Steam Controller's left trackpad, back buttons and capacitive touch reaching a game.
- [`steam-controller-held-by-steam.md`](steam-controller-held-by-steam.md) — finding a Steam Controller that Steam already holds, by its HID device.

## Platform

- [`survive-daemon-restart.md`](survive-daemon-restart.md) — keeping virtual pads across a daemon restart.
- [`inputplumber.md`](inputplumber.md) — cooperating with InputPlumber.
- [`flatpak.md`](flatpak.md) — a Flatpak and Flathub.
- [`virtual-pad-permissions.md`](virtual-pad-permissions.md) — access to virtual pads for other users or sandboxes.
- [`appstream-metainfo-and-icon.md`](appstream-metainfo-and-icon.md) — the metainfo a software center reads, and an icon of CtrlAssist's own.
- [`runtime-crate.md`](runtime-crate.md) — a crate for the paths, durable write and logging the tray and the window take from the daemon crate.
- [`hider-rebind-options.md`](hider-rebind-options.md) — the two ways an unprivileged hiding service can have a pad's driver re-bound, and the root kept and shrunk that was set aside.
- [`uhid-behind-the-service.md`](uhid-behind-the-service.md) — the hiding service could make the virtual DualSense so the seat user needs no uhid grant; the form that closes the risk puts a process in every report's path.
- [`hider-reload-on-upgrade.md`](hider-reload-on-upgrade.md) — a reload that has the hiding service exit once its last connection closes, so an upgrade shows no hidden pad.
- [`steam-blacklist-consent.md`](steam-blacklist-consent.md) — a way to keep CtrlAssist out of Steam's config.
- [`more-identities.md`](more-identities.md) — more virtual pad identities.
- [`virtual-steam-controller.md`](virtual-steam-controller.md) — a virtual Steam Controller identity: what other projects have shown, what Steam does with one, what it would take.
- [`custom-modes.md`](custom-modes.md) — documenting custom modes.

## Feedback

- [`rumble-routing-model.md`](rumble-routing-model.md) — the alternatives to level-based rumble routing and the prior art behind the choice, kept in case it is revisited.
- [`steam-controller-haptics.md`](steam-controller-haptics.md) — a Haptics choice of its own beside Rumble, for a trackpad's clicks to go to whoever touches it.
