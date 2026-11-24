# Ideas

Deferred work and designs set aside, one file each. An entry says what the idea is, why it was deferred, and what would bring it back.


## Clients

- [`gui.md`](gui.md) — a settings window.
- [`tui.md`](tui.md) — a terminal interface.
- [`graph-editor.md`](graph-editor.md) — editing a graph visually.
- [`switch-chords.md`](switch-chords.md) — a button chord as the switch.

## Pads

- [`non-gamepad-pads.md`](non-gamepad-pads.md) — wheels, flight sticks and levers.
- [`pad-nicknames.md`](pad-nicknames.md) — a name the user gives a pad.
- [`profile-new.md`](profile-new.md) — a preset profile from command-line flags.
- [`choice-descriptions.md`](choice-descriptions.md) — a choice's own sentence, where it is chosen.
- [`restart-warning.md`](restart-warning.md) — saying that a game may need restarting after its virtual pad is replaced.
- [`relative-controls.md`](relative-controls.md) — gyro, touchpad, mouse and keyboard.
- [`larger-pad-state.md`](larger-pad-state.md) — more than 64 buttons or 16 axes.
- [`split-preset.md`](split-preset.md) — a preset for splitting one pad into several.

## Platform

- [`survive-daemon-restart.md`](survive-daemon-restart.md) — keeping virtual pads across a daemon restart.
- [`inputplumber.md`](inputplumber.md) — cooperating with InputPlumber.
- [`flatpak.md`](flatpak.md) — a Flatpak and Flathub.
- [`virtual-pad-permissions.md`](virtual-pad-permissions.md) — access to virtual pads for other users or sandboxes.
- [`hider-rebind-options.md`](hider-rebind-options.md) — the two ways an unprivileged hiding service can have a pad's driver re-bound, and the root kept and shrunk that was set aside.
- [`uhid-behind-the-service.md`](uhid-behind-the-service.md) — the hiding service could make the virtual DualSense so the seat user needs no uhid grant; the form that closes the risk puts a process in every report's path.
- [`hider-reload-on-upgrade.md`](hider-reload-on-upgrade.md) — a reload that has the hiding service exit once its last connection closes, so an upgrade shows no hidden pad.
- [`steam-blacklist-consent.md`](steam-blacklist-consent.md) — a way to keep CtrlAssist out of Steam's config.
- [`more-identities.md`](more-identities.md) — more virtual pad identities.
- [`custom-modes.md`](custom-modes.md) — documenting custom modes.

## Feedback

- [`rumble-routing-model.md`](rumble-routing-model.md) — the alternatives to level-based rumble routing and the prior art behind the choice, kept in case it is revisited.
