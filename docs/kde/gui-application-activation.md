# GUI application activation on KDE Plasma 6 Wayland

`ctrlassist-gui` integrates with desktop launchers (app menu, KRunner, `.desktop` entry, the tray's "Settings" button) via the freedesktop `org.freedesktop.Application` D-Bus interface. This doc captures the non-obvious behaviors the handshake relies on — things that were discovered through live testing on KDE Plasma 6 Wayland and aren't apparent from reading the spec alone.

## Activation flow overview

When a user launches the GUI from any entry point:

1. KIO's `DBusActivationRunner` (`kio/src/gui/dbusactivationrunner.cpp`) opens a D-Bus call to `org.freedesktop.Application.Activate` on `io.github.ctrlassist.Gui`.
2. If the GUI isn't running, `dbus-daemon` auto-spawns it via the `.service` file at `share/dbus-1/services/io.github.ctrlassist.Gui.service` and queues the call.
3. The GUI process registers its `Application` interface, claims the bus name, and responds to the queued call (the order matters; see below).
4. The `Activate` handler dispatches on window-existence state: cold-spawn (apply the activation token at window creation) or re-activation (pulse the taskbar).

Implementation is split across:

- [`crates/ctrlassist-gui-app/src/dbus.rs`](../../crates/ctrlassist-gui-app/src/dbus.rs) — `try_claim_and_serve` (bus claim + interface registration), `ApplicationService::raise` (Activate dispatch).
- [`crates/ctrlassist-gui-app/src/lib.rs`](../../crates/ctrlassist-gui-app/src/lib.rs) — `BackendSelector::with_winit_window_attributes_hook` consumes the cold-spawn token at `AppWindow::new()` time.
- [`crates/ctrlassist-tray/src/settings.rs`](../../crates/ctrlassist-tray/src/settings.rs) — `open` calls the GUI's `Application.Activate` with empty `platform_data` (the tray is surfaceless, see below).

## Register-before-claim ordering invariant

`dbus-daemon` queues `Activate` calls during cold-spawn auto-activation: the moment we call `request_name`, the daemon resolves the destination to our unique connection name and starts delivering messages. If our object server doesn't have the `Application` interface registered yet, zbus drops the call — its own warning describes this exactly:

> Requesting name `X` before setting up the object server. Method calls arriving before interfaces are registered may be lost.

Live testing in TisVlay (Plasma 6 Wayland, May 2026) confirmed it: claim-first → register-later gave KIO a 25 s `Did not receive a reply` dialog and a "Remote peer disconnected" Plasma notification on subsequent window close.

**The fix is the order in `try_claim_and_serve`**: connect → register interfaces → claim name. By the time we own the name, the object server is ready; queued calls hit the handler immediately.

**Source-of-truth pointers** for re-verification when zbus or KIO move:

- KIO launcher: `~/ws/lgtv/libs/kio/src/gui/dbusactivationrunner.cpp::startProcess`. Object-path derivation, interface name, the three methods called (`Activate` / `Open` / `ActivateAction`), and the `activation-token` packing from the spawn-process env all live here.
- zbus warning emitter: `~/ws/lgtv/libs/zbus/zbus/src/connection/mod.rs:633`. The warning condition checks `is_bus() && object_server.get().is_none()` — we suppress it by touching the object server (via `at(...)`) before `request_name_with_flags`.
- zbus's atomic builder alternative (`Builder::session().name().serve_at().build()`) was rejected because it forces `DoNotQueue` and errors on conflict, killing our graceful-proxy-on-Exists path.

## KIO's `activation-token` behavior

`DBusActivationRunner::startProcess` reads `XDG_ACTIVATION_TOKEN` from the **spawn process's** environment and includes it in `platform_data` *unconditionally* — even when the env var is unset (sent as empty string). KDE only populates that env var when the launch context has a token (Plasma kickoff with a focused surface, KRunner with seat focus, etc.); from a surfaceless context like a tray icon or a `gdbus` shell, the var is empty.

Two consequences:

1. **The tray doesn't acquire its own token.** An earlier revision of TisVlay's tray tried to obtain a tray-side `xdg_activation_v1` token via direct Wayland-client calls. KWin almost always denied it (no `set_surface()` possible — ksni's surface isn't reachable from the tray process), so the token came back empty in practice. Its tray, and `ctrlassist-tray`'s `settings::open` copied from it, pass `HashMap::new()` for `platform_data`, and the result is identical.

2. **KIO doesn't observe token contents — only Activate-reply success.** `DBusPendingCallWatcher::finished` emits `processStarted` whenever the Activate call succeeds, regardless of whether `platform_data` was empty. Empty tokens silence the "Remote peer disconnected" notification just as well as full tokens.

## Cold-spawn vs re-activation dispatch

The `Activate` handler routes on `Activation::window.get()` presence:

- **Cold-spawn path** (window not yet constructed): stash the inbound `activation-token` in `Activation::cold_spawn_token: Arc<Mutex<Option<String>>>`. `BackendSelector::with_winit_window_attributes_hook` in `lib.rs` reads the slot during `AppWindow::new()` and applies it via `WindowAttributesExtStartupNotify::with_activation_token`. winit then issues the `xdg_activation_v1.activate` request on its own surface when it maps, completing the launcher's startup-notification handshake.

- **Re-activation path** (window exists): the handler invokes `slint::invoke_from_event_loop` with the captured `slint::Weak<AppWindow>`, and inside the closure calls `app.window().with_winit_window(|w| w.request_user_attention(Some(UserAttentionType::Informational)))`. winit's Wayland backend issues an `xdg_activation_v1` request bound to our surface with `XdgActivationTokenData::Attention(...)` payload.

Both paths require slint's `unstable-winit-030` feature — see [docs/concerns/slint-unstable-winit-030.md](../concerns/slint-unstable-winit-030.md) for the pin and audit checklist.

`UserAttentionType::Critical` is intentionally not used — `Critical` is for unrecoverable errors and unread-message urgency; using it for a launcher re-fire would be screen pollution.

## KWin's FSP → `demands_attention` mapping

KWin maps `Attention`-flagged xdg-activation-v1 tokens onto the `demands_attention` window state, then applies the user's Focus Stealing Prevention policy:

- **Extreme / High FSP** (default on Plasma 6): the taskbar entry pulses; the window is not raised.
- **Low / Medium FSP**: the window is raised directly.
- **None**: the window steals focus immediately (rare; non-default).

This is compositor configuration, not a Slint or winit setting. The `request_user_attention` API contract is "request attention," not "raise." The taskbar pulse under default FSP is the spec-compliant UX.

## `gdbus` smoke-test caveat

```sh
gdbus call --session --dest io.github.ctrlassist.Gui --object-path /io/github/ctrlassist/Gui \
    --method org.freedesktop.Application.Activate '{}'
```

This is fine for verifying the **spawn** path under D-Bus auto-activation: kill any running instance, run the command, watch a fresh GUI window appear.

It is **not** a valid smoke test for the **raise-existing-window** path. `gdbus` from a non-Wayland-surface shell can't obtain a token, so even when the call succeeds the window may not pulse under default KWin FSP (and won't raise at all). For end-to-end "raise an existing window" verification, click the tray's "Settings" button or use the app menu / KRunner entry — those go through the GUI's own `request_user_attention` path which doesn't need a launcher-supplied token.

## Open questions

- **Pulse persistence across rapid clicks.** KWin clears `demands_attention` once the user focuses the window. Repeated tray clicks (re-fires of Activate) may escalate to over-pulse / flicker on edge cases. Not observed in practice; debounce in the handler if it surfaces.
- **GNOME Shell behavior.** The same code path should produce GNOME's running-app-indicator pulse via the same xdg-activation-v1 mechanism. Validate on a GNOME session if available; "should work" hypothesis only — we ship for KDE Plasma 6 Wayland.
