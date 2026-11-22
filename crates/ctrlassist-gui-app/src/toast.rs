//! Top-center toast banner. Auto-clears after a short timeout;
//! coalesces across rapid calls so only the most recent message
//! stays visible.

use std::sync::atomic::{AtomicU64, Ordering};

use slint::{ComponentHandle, SharedString};

use crate::{AppWindow, Toast, on_ui};

/// Visible duration of a toast before it auto-clears, in milliseconds.
/// Tuned long enough to read a short success line and short enough
/// not to linger across the user's next action.
const TOAST_DURATION_MS: u64 = 2500;

/// Monotonic generation token for in-flight toasts. Each `show_toast`
/// call increments it; the spawned auto-clear task captures its
/// generation and only clears the message if the counter still
/// matches at wakeup. Means a rapid second toast replaces the first
/// without the first's clear-task wiping out the second prematurely.
static TOAST_GEN: AtomicU64 = AtomicU64::new(0);

/// Display a top-center toast for `TOAST_DURATION_MS` and auto-clear.
/// Safe to call from any thread; marshals through the Slint event
/// loop. Multiple rapid calls coalesce — only the most recent
/// message stays visible, no toast queue.
pub(crate) fn show_toast(app_weak: slint::Weak<AppWindow>, message: &str) {
    let generation = TOAST_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let message = message.to_string();
    on_ui(&app_weak, move |app| {
        app.global::<Toast>()
            .set_message(SharedString::from(message));
    });
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(TOAST_DURATION_MS)).await;
        if TOAST_GEN.load(Ordering::SeqCst) == generation {
            on_ui(&app_weak, |app| {
                app.global::<Toast>().set_message(SharedString::default());
            });
        }
    });
}
