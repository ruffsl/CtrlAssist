//! The Pads pane: its rows from the daemon's mirror, and its two
//! actions.

use std::time::Duration;

use ctrlassist::{DaemonClient, Pad, Setup};
use slint::{ComponentHandle, Model as _, ModelRc, SharedString, VecModel};
use tracing::warn;

use crate::{AppWindow, ErrorState, Errors, PadRow, Pads, on_ui};

/// How long Identify buzzes a pad: the command line's default.
const IDENTIFY: Duration = Duration::from_millis(600);

/// The bus numbers the kernel gives the two a pad is usually on.
const BUS_USB: u16 = 3;
const BUS_BLUETOOTH: u16 = 5;

/// A pad's id made fit for an `accessible-id`: its letters and digits,
/// and a hyphen for anything else.
fn key_of(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// The pad's address, when that is what it is known by.
fn address_of(id: &str) -> &str {
    id.strip_prefix("address:").unwrap_or_default()
}

/// A few characters that tell this pad from another of its model: the
/// end of whatever it is known by, when that is its own (an address, a
/// serial number, a port) and not just its model.
fn tag_of(id: &str) -> String {
    if id.starts_with("model:") {
        return String::new();
    }
    let own = id.split_once(':').map_or(id, |(_, own)| own);
    let tail: Vec<char> = own.chars().rev().take(5).collect();
    tail.into_iter().rev().collect()
}

/// The pane's rows: connected pads first, then the remembered ones,
/// each group in name order and then by id, so two of one name keep
/// their places.
#[must_use]
pub fn rows(pads: &[Pad], setups: &[Setup]) -> Vec<PadRow> {
    let mut sorted: Vec<&Pad> = pads.iter().collect();
    sorted.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.id.cmp(&b.id))
    });
    sorted
        .into_iter()
        .map(|pad| {
            // Its role is the name its setup's profile gives the source
            // it fills.
            let role = pad
                .setup
                .as_deref()
                .and_then(|name| setups.iter().find(|setup| setup.name == name))
                .and_then(|setup| {
                    setup
                        .sources
                        .iter()
                        .find(|source| source.pad.as_deref() == Some(pad.id.as_str()))
                })
                .map(|source| source.name.clone())
                .unwrap_or_default();
            PadRow {
                id: SharedString::from(pad.id.as_str()),
                key: SharedString::from(key_of(&pad.id)),
                name: SharedString::from(pad.name.as_str()),
                tag: SharedString::from(tag_of(&pad.id)),
                model: SharedString::from(format!("{:04x}:{:04x}", pad.vendor, pad.product)),
                connection: SharedString::from(match pad.bus {
                    Some(BUS_USB) => "USB",
                    Some(BUS_BLUETOOTH) => "Bluetooth",
                    _ => "",
                }),
                connected: pad.connected,
                rumble: SharedString::from(match pad.rumble {
                    Some(true) => "Yes",
                    Some(false) => "No",
                    None => "",
                }),
                setup: SharedString::from(pad.setup.clone().unwrap_or_default()),
                role: SharedString::from(role),
                hidden: pad.hidden,
                address: SharedString::from(address_of(&pad.id)),
            }
        })
        .collect()
}

/// Puts the rows on the window, keeping the selection on the pad it was
/// on: rows move as pads come and go, and the pad selected should stay
/// selected wherever its row lands. One that is gone leaves nothing
/// selected.
pub fn show(app: &AppWindow, rows: Vec<PadRow>) {
    let pads = app.global::<Pads>();
    let selected = pads
        .get_rows()
        .row_data(usize::try_from(pads.get_selected_index()).unwrap_or(usize::MAX))
        .map(|row| row.id);
    let index = selected
        .and_then(|id| rows.iter().position(|row| row.id == id))
        .and_then(|at| i32::try_from(at).ok())
        .unwrap_or(-1);
    let connected = rows.iter().filter(|row| row.connected).count();
    pads.set_connected_count(i32::try_from(connected).unwrap_or(i32::MAX));
    pads.set_rows(ModelRc::new(VecModel::from(rows)));
    pads.set_selected_index(index);
}

/// The pane's banner for an action the daemon refused.
fn refused(what: &str, error: &ctrlassist::Error) -> ErrorState {
    ErrorState {
        kind: SharedString::from("pads"),
        tone: SharedString::from("error"),
        title: SharedString::from(what),
        message: SharedString::from(error.to_string()),
        action_label: SharedString::default(),
    }
}

/// Binds Identify and Forget. What either changes comes back through
/// the mirror, as any change does.
pub(crate) fn wire(app: &AppWindow, daemon: &DaemonClient) {
    let pads = app.global::<Pads>();
    // To the Setups pane. Which setup it shows comes with that pane.
    let weak = app.as_weak();
    pads.on_open_setup(move |_setup| {
        if let Some(app) = weak.upgrade() {
            app.invoke_navigate(crate::Pane::Setups);
        }
    });
    let (weak, client) = (app.as_weak(), daemon.clone());
    pads.on_identify(move |id| {
        let (weak, client, id) = (weak.clone(), client.clone(), id.to_string());
        tokio::spawn(async move {
            let banner = match client.identify_pad(&id, IDENTIFY).await {
                Ok(()) => ErrorState::default(),
                Err(error) => {
                    warn!(%error, pad = id, "the pad couldn't be buzzed");
                    refused("The pad couldn't be buzzed", &error)
                }
            };
            on_ui(&weak, move |app| app.global::<Errors>().set_pads(banner));
        });
    });
    let (weak, client) = (app.as_weak(), daemon.clone());
    pads.on_forget(move |id| {
        let (weak, client, id) = (weak.clone(), client.clone(), id.to_string());
        tokio::spawn(async move {
            let banner = match client.forget_pad(&id).await {
                Ok(()) => ErrorState::default(),
                Err(error) => {
                    warn!(%error, pad = id, "the pad couldn't be forgotten");
                    refused("The pad couldn't be forgotten", &error)
                }
            };
            on_ui(&weak, move |app| app.global::<Errors>().set_pads(banner));
        });
    });
}
