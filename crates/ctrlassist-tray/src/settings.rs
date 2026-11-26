//! Opening the settings window from the tray.
//!
//! The window answers `org.freedesktop.Application.Activate` on the
//! session bus under its own name: the bus starts it for the call, from
//! the service file it is installed with, and a window already open
//! comes to the front.

use std::collections::HashMap;

use anyhow::{Context, Result};
use zbus::zvariant::Value;

/// The window's name on the session bus, and its application id.
const NAME: &str = "io.github.ctrlassist.Gui";
const PATH: &str = "/io/github/ctrlassist/Gui";

/// Opens the window, or raises the one that is open.
pub(crate) async fn open() -> Result<()> {
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
        .context("asking the session bus for the window; is ctrlassist-gui installed?")?;
    Ok(())
}
