//! Opening the settings window from the tray.
//!
//! The window answers `org.freedesktop.Application.Activate` on the
//! session bus under its own name: where it is installed with its
//! service file the bus starts it for the call, and a window already
//! open comes to the front. Where the bus knows no such service, as
//! from a build tree, the window's binary is started from beside this
//! one.

use std::collections::HashMap;

use anyhow::{Context, Result};
use zbus::zvariant::Value;

/// The window's name on the session bus, and its application id.
const NAME: &str = "io.github.ctrlassist.Gui";
const PATH: &str = "/io/github/ctrlassist/Gui";

/// Opens the window, or raises the one that is open.
pub(crate) async fn open() -> Result<()> {
    match activate().await {
        Ok(()) => Ok(()),
        Err(activation) => start_beside().with_context(|| {
            format!("the session bus couldn't start it ({activation:#}), and nor could the tray")
        }),
    }
}

async fn activate() -> Result<()> {
    let bus = zbus::Connection::session()
        .await
        .context("connecting to the session bus")?;
    let window = zbus::Proxy::new(&bus, NAME, PATH, "org.freedesktop.Application")
        .await
        .context("addressing the window on the session bus")?;
    let platform_data: HashMap<String, Value<'_>> = HashMap::new();
    let _: () = window
        .call("Activate", &(platform_data,))
        .await
        .context("asking the window to open")?;
    Ok(())
}

/// Starts `ctrlassist-gui` from the directory this binary is in, and
/// leaves it to run. A thread waits for it, so that a window that is
/// closed doesn't stay in the process table for as long as the tray
/// runs.
fn start_beside() -> Result<()> {
    let beside = std::env::current_exe()
        .context("finding where the tray runs from")?
        .with_file_name("ctrlassist-gui");
    let mut window = std::process::Command::new(&beside)
        .spawn()
        .with_context(|| format!("starting {}", beside.display()))?;
    std::thread::spawn(move || {
        let _ = window.wait();
    });
    Ok(())
}
