## Slint `ComboBox` dropdown clipped by the window

**Priority**: low (resolved app-wide via `ThemedSelect`; kept open only to track the upstream revert)
**Found by**: TisVlay's GUI review (July 2026), reproduced there live via the Slint MCP server; copied with the window, whose dropdowns are TisVlay's `ThemedSelect`
**Status**: resolved — the window has no stock `ComboBox`: every dropdown (the profile form's fields in `profiles_pane.slint`, couch mode in `settings_pane.slint`) is the custom flip-up `ThemedSelect`, so no stock `PopupWindow` can clip. `ThemedSelect` is `Theme`-tokened (matches the rest of the app + the DE accent). Remove it and revert to stock `ComboBox` when upstream native popups land.

### Reproduced, in TisVlay

Confirmed on TisVlay's running GUI (KDE Plasma Wayland, display scale 2.0; window 1440×960 physical = 720×480 logical), driven through the Slint MCP server:

- A field editor listing 7 `ComboBox` rows, the last sitting at the bottom edge (logical y≈435 in a 480-tall window).
- Opening it drops a **208px-logical (416px-physical)** popup **downward**. ~160px of it falls below the window's bottom edge and is clipped — only the first option is visible.
- The popup is **not** repositioned upward, and the stock `ComboBox` popup's internal `ScrollView` is itself clipped to the visible sliver, so the hidden options are effectively **unreachable** without first scrolling the field list to move the row away from the bottom edge.

CtrlAssist's profile form has the same shape: a column of dropdown rows in a scrollable detail, at a 720 px minimum width.

### Context

Slint's winit backend has no real popup surface: winit itself never implemented one ([rust-windowing/winit#950](https://github.com/rust-windowing/winit/issues/950)), so Slint *simulates* a `PopupWindow` — the surface backing a `ComboBox` dropdown, and also the 1.17 `Tooltip` — by drawing it *within* the top-level window. A `ComboBox` near a window edge therefore clips to the window bounds instead of overflowing past them like a native menu. (Slint's Qt backend renders popups as true OS windows and doesn't have this limitation; `ctrlassist-gui` uses the winit + femtovg backend.)

There *is* an upstream clamp — [slint-ui/slint#6434](https://github.com/slint-ui/slint/pull/6434) (Nov 2024) added `place_popup()`, which is supposed to reposition the popup to stay inside the window when it wouldn't otherwise fit (`i-slint-core`'s `window.rs::show_popup` → `popup::place_popup`, with the window rect as the clip region). **But the live repro above shows it does not take effect for a winit-Wayland `ComboBox` popup** — the dropdown clips downward rather than shifting up. So this is still the #2375 behavior in practice. It is **not** a scale-factor artifact: the clip reproduced identically at KDE display scale 100% and 200%. Why the clamp doesn't fire on the ChildWindow path is an open question — a focused upstream repro would be needed to pin it (candidates: the clamp clips size but not origin on this path, or the ChildWindow render ignores the clamped origin).

### Upstream tracking

Root cause and the live fix path (verified by reading the full threads, July 2026):

- [slint#2375](https://github.com/slint-ui/slint/issues/2375) — *"PopupWindow is clipped by Window"* (open; root cause). Also the home of a separate ComboBox-popup-scrolling request.
- [slint#12260](https://github.com/slint-ui/slint/issues/12260) — *"Tooltip is clipped when the anchor widget is near the edge of the window"* (open; same limitation, tooltip surface).
- [slint#11740](https://github.com/slint-ui/slint/pull/11740) — *"Native winit popup"* (open **draft**). Depends on winit#4543, `Fixes` #12260, uses a ChildWindow fallback where native popups aren't supported. Wayland demo already working.
- [winit#4543](https://github.com/rust-windowing/winit/pull/4543) — *"Wayland, Windows, MacOS: Popup Implementation"* (open, **not draft**; actively developed, NLnet-sponsored). Adds `WindowAttributes::with_type()` for decorationless child popups. **Wayland (Mutter + KDE Plasma), Windows, macOS done; X11 pending.** This is the PR slint#11740 tracks.
- [winit#4444](https://github.com/rust-windowing/winit/pull/4444) — *"Another attempt at adding support for popups"* (open **draft**, GTK-style `create_popup` API). An **earlier, superseded** attempt, stale since March 2026 — winit#4543 is the one that carried forward.

Notably, winit#4543's tested platforms include **KDE Plasma on Wayland**, the window's primary target, and the window is Wayland-only, so X11's pending support doesn't hold it back.

### Risk

Low: no stock `ComboBox` remains. A stock one added later would clip near the window's bottom edge, and its hidden options can't be scrolled into view within the clipped popup (Slint blocks scroll while a popup is open — an open `PopupWindow` is pointer-modal, the same modality native Qt/KDE combos have), so the workaround would be to enlarge the window. No data-corruption risk; the current selection and the visible options still commit correctly.

### Mitigation

- **Flip-up overlay (`ThemedSelect`, applied app-wide).** `crates/ctrlassist-gui/ui/globals/select.slint` holds `ThemedSelect` — a drop-in for `ComboBox` on `model`/`current-value`/`selected` — whose dropdown is an in-window overlay (`SelectOverlay` at the AppWindow root), **not** a Slint `PopupWindow`. Mirroring the [Help overlay](../../crates/ctrlassist-gui/ui/globals/help.slint), it escapes the detail's clipping ScrollView, reads the window size, and **flips upward when the row sits low in the window** — matching native Qt/KDE. Selection + keyboard (arrow/enter/escape, modal-while-open, focus-visible ring) are re-implemented since `ComboBoxBase` isn't exported. Colors are `Theme`-tokened so it matches the rest of the app (and the DE accent).
- **Qt backend (rejected).** Slint's Qt backend renders real popup windows and sidesteps this entirely — but adopting it swaps the whole rendering path and pulls a Qt dependency, disproportionate here.

### Re-evaluation triggers

- winit#4543 merges → watch slint#11740 to consume it, then bump Slint (coordinate with the exact-version pin — see [Slint `unstable-winit-030` feature dependency](slint-unstable-winit-030.md)).
- A Slint release closes [#2375](https://github.com/slint-ui/slint/issues/2375) or ships #11740 → drop this concern, delete `select.slint`, revert the dropdowns to stock `ComboBox`, and re-verify a dropdown at the window's bottom edge. Do it with TisVlay, whose `ThemedSelect` this is.
- If worth chasing upstream: file a focused repro against #2375 showing #6434's `place_popup` clamp not repositioning a `ComboBox` popup on the winit-Wayland ChildWindow path.
