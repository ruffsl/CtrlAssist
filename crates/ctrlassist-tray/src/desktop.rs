//! The parts of the tray that need a desktop session: the session bus
//! for the tray item and for notifications. Everything they call is
//! tested without one; these are run by hand, on a desktop.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use ctrlassist::DaemonClient;
use ksni::TrayMethods;
use notify_rust::Notification;
use tokio::sync::{Notify, mpsc};
use tracing::{debug, info, warn};

use crate::notify::{self, NotificationSettings, Notifier};
use crate::tray::PadTray;
use crate::{Handle, act, follow};

/// Shows each toast on the desktop, through the session bus's
/// notification daemon. One at a time: a burst while no notification
/// daemon answers is a row of short failures, not as many stalls.
fn show_toasts(initial: NotificationSettings) -> Notifier {
    let (notifier, mut toasts) = Notifier::capturing(initial);
    tokio::spawn(async move {
        while let Some(toast) = toasts.recv().await {
            let shown = Notification::new()
                .appname("CtrlAssist")
                .summary(&toast.summary)
                .body(&toast.body)
                .icon(toast.icon)
                .show_async()
                .await;
            if let Err(error) = shown {
                debug!(%error, summary = toast.summary, "a notification couldn't be shown");
            }
        }
    });
    notifier
}

/// What the tray needs to be told.
#[derive(Clone, Debug)]
pub struct TrayOptions {
    /// The daemon's socket.
    pub socket: PathBuf,
    /// The tray's own file, `tray.toml`: its notification switches.
    pub config: PathBuf,
}

/// Runs the tray until it is interrupted or quit from its menu.
///
/// # Errors
///
/// Fails if the session on the daemon's socket can't be set up. A
/// daemon that isn't running is not a failure: the tray waits for it.
pub async fn run(options: TrayOptions) -> Result<()> {
    let daemon = DaemonClient::connect(&options.socket)
        .await
        .map_err(|error| anyhow!("connecting to {}: {error}", options.socket.display()))?;
    let (actions, clicked) = mpsc::unbounded_channel();
    let notifier = show_toasts(
        notify::load_settings(&options.config).unwrap_or_else(|error| {
            warn!(
                error = format!("{error:#}"),
                "the defaults are used instead"
            );
            NotificationSettings::default()
        }),
    );
    // Before the icon, which it doesn't need: the window reaches the
    // tray's switches on a desktop with no StatusNotifier host too.
    tokio::spawn(notify::serve_while_daemon(
        daemon.clone(),
        options.socket.clone(),
        options.config.clone(),
        notifier.clone(),
    ));
    // With no StatusNotifier host the spawn still succeeds: the icon
    // registers if a host appears later, and notifications work meanwhile.
    // Only a failure of the session bus itself ends up here.
    let mut tray = PadTray::new(actions);
    // The profile last used before this start, if it was kept.
    tray.target = notify::load_target(&options.config);
    let handle = match tray.assume_sni_available(true).spawn().await {
        Ok(handle) => Handle::Desktop(handle),
        Err(error) => {
            warn!(%error, "the tray icon can't be shown; nothing to do but wait");
            tokio::signal::ctrl_c()
                .await
                .context("waiting to be stopped")?;
            return Ok(());
        }
    };
    info!(socket = %options.socket.display(), "the tray is up");
    let quit = Arc::new(Notify::new());
    tokio::spawn(follow(daemon.clone(), handle.clone(), notifier.clone()));
    tokio::spawn(act(
        daemon,
        clicked,
        notifier,
        Arc::clone(&quit),
        Some(options.config.clone()),
    ));
    tokio::select! {
        outcome = tokio::signal::ctrl_c() => outcome.context("waiting to be stopped")?,
        () = quit.notified() => {}
    }
    info!("the tray is stopping");
    Ok(())
}
