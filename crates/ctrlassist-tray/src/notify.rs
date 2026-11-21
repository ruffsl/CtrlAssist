//! Where the tray's notifications go: a queue, which the desktop side
//! shows one at a time and tests read instead.

use tokio::sync::mpsc;

use crate::state::Toast;

/// Where the tray's notifications go.
#[derive(Clone, Debug)]
pub struct Notifier {
    tx: mpsc::UnboundedSender<Toast>,
}

impl Notifier {
    /// A notifier whose toasts arrive on the returned receiver instead of
    /// the desktop: what tests assert on, with nothing popping up.
    #[must_use]
    pub fn capturing() -> (Self, mpsc::UnboundedReceiver<Toast>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { tx }, rx)
    }

    /// Shows a toast.
    pub fn notify(&self, toast: Toast) {
        let _ = self.tx.send(toast);
    }
}
