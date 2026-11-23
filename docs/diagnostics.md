# Diagnostic bundle

The settings window's About pane has a Diagnostics button that copies a JSON bundle to the clipboard, to paste into a bug report (the issue form has a field for it). It is a snapshot of what the window sees: the releases of CtrlAssist's parts, the machine, the known pads, the setups and the profiles, enough to answer most "what's your setup?" questions without asking. After TisVlay's diagnostic blob, with CtrlAssist's own sections.

The renderer is [`crates/ctrlassist-gui-app/src/diagnostic.rs`](../crates/ctrlassist-gui-app/src/diagnostic.rs). It takes plain data (`Inputs`), no Slint types, so its tests need no event loop; `about.rs` gathers that data from the About rows and the SDK's mirror at the click.

## Schema (1)

```json
{
  "schema_version": 1,
  "window": { "version": "0.5.0", "profile": "release", "target": "x86_64-unknown-linux-gnu" },
  "parts": {
    "daemon": { "version": "0.5.0", "running": true, "says": "0.5.0" },
    "hider": { "version": "0.5.0", "running": true, "says": "0.5.0" },
    "tray": { "version": "0.5.0", "running": true, "says": "0.5.0" },
    "gui": { "version": "0.5.0", "running": true, "says": "0.5.0" },
    "cli": { "…": "…" }
  },
  "daemon": { "version": "0.5.0", "realtime": true, "hiding": "service", "config": true, "hider_version": "0.5.0" },
  "machine": {
    "kernel": "6.18.24",
    "os_id": "nixos",
    "os_version": "26.05",
    "desktop": "kde",
    "session_type": "wayland",
    "uinput_writable": true,
    "uhid_writable": true,
    "steam_running": false
  },
  "pads": ["…the known pads, as the daemon publishes them"],
  "setups": ["…the setups that exist"],
  "profiles": ["…the kept profiles, with their text"]
}
```

`schema_version` is there so a reader goes by the version rather than guessing from what fields are present; it is bumped when a field is renamed or removed.

## Where each field comes from

| Path | Source |
|---|---|
| `window.version` | the window's own `CARGO_PKG_VERSION` |
| `window.profile` | `debug` or `release`, by `debug_assertions` |
| `window.target` | the target triple, set by `ctrlassist-gui-app`'s build script |
| `parts.<name>` | the About pane's rows, one per part: its release, whether it is there, and what its chip says |
| `daemon` | the daemon's published state, field by field: its release, whether real-time scheduling was granted, the hiding mechanism, and the hiding service's release; `null` with no daemon |
| `daemon.config` | whether the daemon named the directory it keeps the user's files in: a bool, the path itself being left out |
| `machine.kernel` | `/proc/sys/kernel/osrelease` |
| `machine.os_id`, `machine.os_version` | `ID` and `VERSION_ID` in `/etc/os-release`, read as written, so NixOS's `26.05` stays `26.05` |
| `machine.desktop`, `machine.session_type` | `XDG_CURRENT_DESKTOP` and `XDG_SESSION_TYPE`, lowercased; `null` when unset or empty |
| `machine.uinput_writable`, `machine.uhid_writable` | whether this user can open `/dev/uinput` and `/dev/uhid` for writing, which every virtual pad, and a virtual DualSense, need |
| `machine.steam_running` | whether a process named `steam` runs |
| `pads`, `setups`, `profiles` | the SDK's mirror of the daemon |

## What is masked or left out

- **Pad addresses are masked everywhere in the text,** after rendering: `0c:27:56:4f:bd:14` becomes `0c:27:56:xx:xx:xx#1`. The vendor half stays, which tells a maker's pad from a clone; the number is the address's place among those met, so the same address reads the same everywhere and two pads of one maker don't read alike.
- **No file paths.** The directory the daemon keeps the user's files in names the home directory, and so usually the user: `daemon.config` says only whether the daemon named one, as TisVlay's bundle does. The path stays in the window, for About's buttons that open the folders. Nor does the bundle carry the tray's or the window's settings file, the daemon's state directory, or a socket under `/run/user/<uid>`. What it does carry that is a path is a virtual pad's event node, `/dev/input/event…`, in `setups`, and in a failed setup's reason the node of the pad it couldn't open: those name no user.
- **No host identity.** Hostname and user name are not collected.
- **The profiles' text is the user's own,** comments included, and goes in as written, addresses masked. The issue form's logs field reminds them the report is public.
