//! The key router: what the shell's key handling asks Rust for, and
//! what Rust watches on its behalf.
//!
//! Slint offers no "focus next" call, in `.slint` or in Rust, so Up and
//! Down synthesize a Tab or Backtab through the window and let Slint's
//! own traversal move focus. The dispatch is synchronous and nested in
//! the arrow key's handler; `Nav.synthesizing` marks it so the shell's
//! own Tab handling (couch mode's pane cycle) lets it through.
//!
//! The detail panes navigate by rows (docs/gui/remote-navigation.md):
//! every stop reports the row it sits in as it takes focus, and a row
//! step is a walk along the Tab order until the row changes, undone
//! when the column changes instead (the order wrapped out of the
//! detail), so the detail stops at its ends. The traversal already
//! skips what's disabled or hidden and scrolls the landing into view.
//!
//! Slint also runs no handler at all once the focused item is gone, so
//! a remote's keys would go dead after a pane switch or a refresh that
//! destroyed the focused control. The shell counts every key its capture
//! handler sees; the winit hook notes the count before a navigation key
//! and checks it after the dispatch, and a key that didn't move it asks
//! the shell to restore focus to its last column.

use slint::platform::{Key, WindowEvent};
use slint::{ComponentHandle, SharedString};

use crate::{AppWindow, Nav};

/// The most stops a row walk crosses before giving up: longer than any
/// row, shorter than a pane.
const MAX_HOPS: usize = 40;

/// Bind the shell's `Nav.step`, `Nav.row-step` and `Nav.stop-step` to
/// synthesized Tab walks. The tests bind them too, so a dispatched
/// arrow walks focus there as well.
pub fn bind_key_router(app: &AppWindow) {
    let weak = app.as_weak();
    app.global::<Nav>().on_step(move |forward| {
        if let Some(app) = weak.upgrade() {
            dispatch_tab(&app, forward);
        }
    });
    let weak = app.as_weak();
    app.global::<Nav>().on_row_step(move |forward| {
        if let Some(app) = weak.upgrade() {
            row_step(&app, forward);
        }
    });
    let weak = app.as_weak();
    app.global::<Nav>()
        .on_stop_step(move |forward| weak.upgrade().is_some_and(|app| stop_step(&app, forward)));
}

/// One synthesized Tab (forward) or Backtab, then the deferred change
/// handlers, so a stop that reports its row from one has done so.
fn dispatch_tab(app: &AppWindow, forward: bool) {
    let nav = app.global::<Nav>();
    let text: SharedString = if forward { Key::Tab } else { Key::Backtab }.into();
    nav.set_synthesizing(true);
    app.window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    app.window()
        .dispatch_event(WindowEvent::KeyReleased { text });
    nav.set_synthesizing(false);
    slint::platform::update_timers_and_animations();
}

/// Walk back the given number of hops.
fn undo(app: &AppWindow, forward: bool, hops: usize) {
    for _ in 0..hops {
        dispatch_tab(app, !forward);
    }
}

/// Where focus is: its column and row as the stops report them.
fn position(app: &AppWindow) -> (i32, i32) {
    let nav = app.global::<Nav>();
    (nav.get_column(), nav.get_focus_row())
}

/// Up or Down in the detail: the previous or next row's first stop. A
/// stop in no row (0) steps one stop, as the plain Tab order does.
pub(crate) fn row_step(app: &AppWindow, forward: bool) {
    let (column, row) = position(app);
    if row == 0 {
        // One stop, undone when it left the column: a stop in no row
        // stops at the detail's ends as a row does.
        dispatch_tab(app, forward);
        if position(app).0 != column {
            undo(app, forward, 1);
        }
        return;
    }
    // Leave the row.
    let mut hops = 0;
    loop {
        dispatch_tab(app, forward);
        hops += 1;
        let (c, r) = position(app);
        if c != column || hops >= MAX_HOPS {
            undo(app, forward, hops);
            return;
        }
        if r != row {
            break;
        }
    }
    if forward {
        // Tab landed on the next row's first stop.
        return;
    }
    // Backtab landed on the previous row's last stop: on to its first,
    // one past it and one back. A stop in no row is a row of its own.
    let (_, landed) = position(app);
    if landed == 0 {
        return;
    }
    loop {
        dispatch_tab(app, false);
        hops += 1;
        let (c, r) = position(app);
        if c != column || r != landed {
            dispatch_tab(app, true);
            return;
        }
        if hops >= MAX_HOPS {
            return;
        }
    }
}

/// Right or Left in the detail: the next or previous stop of the same
/// row. False, with focus where it was, when the row has none that
/// way, so the shell jumps columns instead.
pub(crate) fn stop_step(app: &AppWindow, forward: bool) -> bool {
    let (column, row) = position(app);
    if row == 0 {
        return false;
    }
    dispatch_tab(app, forward);
    if position(app) == (column, row) {
        return true;
    }
    dispatch_tab(app, !forward);
    false
}

/// The keys a remote sends that Slint's winit backend drops, as the
/// Slint key they mean; the hook dispatches that key instead. From
/// Plasma Bigscreen's input handler: a gamepad's Share or L2 (KEY_BACK,
/// `XF86Back`) is Escape, and its Square (KEY_MENU, the `XF86MenuKB`
/// keysym winit leaves unidentified) is Menu. From the kernel's CEC
/// remote keymap, when a TV forwards its remote's keys: Exit (KEY_EXIT,
/// `XF86Close`, which winit names but Slint doesn't) is Escape, and
/// Info (KEY_INFO, the `XF86Info` keysym winit leaves unidentified) is
/// Menu.
pub(crate) fn remap_dropped_key(key: &slint::winit_030::winit::keyboard::Key) -> Option<Key> {
    use slint::winit_030::winit::keyboard::{Key as WinitKey, NamedKey, NativeKey};
    const XF86_MENU_KB: u32 = 0x1008_ff65;
    const XF86_INFO: u32 = 0x1008_1166;
    match key {
        WinitKey::Named(NamedKey::BrowserBack | NamedKey::Close) => Some(Key::Escape),
        WinitKey::Unidentified(NativeKey::Xkb(XF86_MENU_KB | XF86_INFO)) => Some(Key::Menu),
        _ => None,
    }
}

/// The shell's key count, taken before a key is dispatched.
pub fn key_seen(app: &AppWindow) -> i32 {
    app.global::<Nav>().get_key_seen()
}

/// After a navigation key was dispatched: if the shell never saw it,
/// nothing had focus, so restore it. Returns whether it did.
pub fn recover_if_unhandled(app: &AppWindow, seen_before: i32) -> bool {
    if key_seen(app) != seen_before {
        return false;
    }
    app.invoke_restore_focus();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::winit_030::winit::keyboard::{Key as WinitKey, NamedKey};

    #[test]
    fn the_dropped_back_and_menu_keys_are_remapped_and_the_rest_is_left_alone() {
        use slint::winit_030::winit::keyboard::NativeKey;
        // The gamepad's, through Bigscreen's input handler.
        assert_eq!(
            remap_dropped_key(&WinitKey::Named(NamedKey::BrowserBack)),
            Some(Key::Escape)
        );
        assert_eq!(
            remap_dropped_key(&WinitKey::Unidentified(NativeKey::Xkb(0x1008_ff65))),
            Some(Key::Menu)
        );
        // The CEC remote's, through the kernel's keymap.
        assert_eq!(
            remap_dropped_key(&WinitKey::Named(NamedKey::Close)),
            Some(Key::Escape)
        );
        assert_eq!(
            remap_dropped_key(&WinitKey::Unidentified(NativeKey::Xkb(0x1008_1166))),
            Some(Key::Menu)
        );
        assert_eq!(remap_dropped_key(&WinitKey::Named(NamedKey::Escape)), None);
        // The keyboard's Menu key arrives named, and the backend maps it itself.
        assert_eq!(
            remap_dropped_key(&WinitKey::Named(NamedKey::ContextMenu)),
            None
        );
        assert_eq!(remap_dropped_key(&WinitKey::Character("a".into())), None);
    }
}
