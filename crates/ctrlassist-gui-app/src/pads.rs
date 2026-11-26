//! The Pads pane: its rows from the daemon's mirror, and its two
//! actions.

use std::time::Duration;

use ctrlassist::{DaemonClient, Pad, Profile, Setup};
use ctrlassist_protocol::profile::{Hide, Plan};
use slint::{ComponentHandle, Model as _, ModelRc, SharedString, VecModel};
use tracing::warn;

use crate::nav::NavHandles;
use crate::{AppWindow, ErrorState, Errors, PadProfileRow, PadRow, Pads, on_ui, status};

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
pub(crate) fn tag_of(id: &str) -> String {
    if id.starts_with("model:") {
        return String::new();
    }
    let own = id.split_once(':').map_or(id, |(_, own)| own);
    let tail: Vec<char> = own.chars().rev().take(5).collect();
    tail.into_iter().rev().collect()
}

/// A pad's maker, for its tile, by its vendor number; empty for one
/// with no mark of its own.
fn maker_of(vendor: u16) -> &'static str {
    match vendor {
        0x054c => "sony",
        0x045e => "microsoft",
        0x057e => "nintendo",
        0x2dc8 => "8bitdo",
        0x28de => "valve",
        _ => "",
    }
}

/// The pane's rows: connected pads first, then the remembered ones,
/// each group in name order and then by id, so two of one name keep
/// their places.
#[must_use]
pub fn rows(
    pads: &[Pad],
    setups: &[Setup],
    profiles: &[Profile],
    identities: &[ctrlassist::Identity],
) -> Vec<PadRow> {
    let mut rows = real_rows(pads, setups, profiles);
    rows.extend(virtual_rows(setups, identities));
    rows
}

/// The id a virtual pad's row goes by: the profile that makes it and
/// its name there, as the daemon's `identify` takes it. No real pad's
/// id starts so.
#[must_use]
pub fn virtual_id(profile: &str, sink: &str) -> String {
    ctrlassist_protocol::rpc::virtual_pad(profile, sink)
}

/// A row for each virtual pad of a running profile, by profile and then
/// by its name there. A virtual pad is there only while its profile
/// runs, so these come and go with the profiles.
fn virtual_rows(setups: &[Setup], identities: &[ctrlassist::Identity]) -> Vec<PadRow> {
    let mut running: Vec<&Setup> = setups.iter().collect();
    running.sort_by(|a, b| a.name.cmp(&b.name));
    running
        .into_iter()
        .flat_map(|setup| setup.sinks.iter().map(move |sink| (setup, sink)))
        .map(|(setup, sink)| {
            let id = virtual_id(&setup.name, &sink.name);
            let known = identities.iter().find(|known| known.id == sink.identity);
            let identity = status::capitalized(status::identity(&sink.identity));
            let node = sink
                .node
                .as_deref()
                .map(|node| node.rsplit('/').next().unwrap_or(node).to_string());
            // It goes by the name a game sees it under, as a real pad
            // does; which profile's it is, is said under that.
            let name = known.map_or_else(|| identity.clone(), |known| known.name.clone());
            let status = status::of(&setup.name, None, false, std::slice::from_ref(setup), &[]);
            PadRow {
                is_virtual: true,
                key: SharedString::from(key_of(&id)),
                id: SharedString::from(id),
                name: SharedString::from(name),
                // A pad that presents itself as nobody else's is
                // CtrlAssist's own.
                maker: SharedString::from(match sink.identity.as_str() {
                    "xbox" => "microsoft",
                    "dualsense" => "sony",
                    _ => "ctrlassist",
                }),
                word: SharedString::from(format!("In {}", setup.name)),
                tone: SharedString::from(if node.is_some() { "success" } else { "neutral" }),
                // What tells it from another of its kind: its node.
                tag: SharedString::from(node.clone().unwrap_or_default()),
                model: SharedString::from(known.map_or_else(String::new, |known| {
                    format!("{:04x}:{:04x}", known.vendor, known.product)
                })),
                identity: SharedString::from(identity),
                node: SharedString::from(sink.node.clone().unwrap_or_default()),
                connection: SharedString::from("Virtual"),
                connected: node.is_some(),
                setup: SharedString::from(setup.name.as_str()),
                role: SharedString::from(status::capitalized(&sink.name)),
                profiles: ModelRc::new(VecModel::from(vec![PadProfileRow {
                    name: SharedString::from(setup.name.as_str()),
                    part: SharedString::from(status::capitalized(&sink.name)),
                    word: SharedString::from(status.word()),
                    tone: SharedString::from(status.tone()),
                }])),
                ..PadRow::default()
            }
        })
        .collect()
}

/// The real pads' rows: connected ones first, then the remembered ones.
fn real_rows(pads: &[Pad], setups: &[Setup], profiles: &[Profile]) -> Vec<PadRow> {
    // Each profile worked out once, one with its pads still to choose
    // among them; one the daemon finds wrong names no pad that can be
    // told.
    let plans: Vec<(&str, Plan, bool)> = profiles
        .iter()
        .filter_map(|profile| {
            let (plan, unset) = status::plan_of(profile)?;
            Some((profile.name.as_str(), plan, unset))
        })
        .collect();
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
            let named: Vec<PadProfileRow> = plans
                .iter()
                .filter_map(|(name, plan, unset)| {
                    let source = plan
                        .sources
                        .iter()
                        .find(|source| status::names(&source.pad, pad))?;
                    let status = status::of(name, Some(plan), *unset, setups, pads);
                    Some(PadProfileRow {
                        name: SharedString::from(*name),
                        part: SharedString::from(status::capitalized(&source.name)),
                        word: SharedString::from(status.word()),
                        tone: SharedString::from(status.tone()),
                    })
                })
                .collect();
            let (word, tone) = match (&pad.setup, pad.connected) {
                (_, false) => ("Remembered".to_string(), "neutral"),
                (Some(setup), true) => (format!("In {setup}"), "success"),
                (None, true) => ("Free".to_string(), "info"),
            };
            // What its setup's profile asks, and what came of it.
            let asked = pad.setup.as_deref().and_then(|setup| {
                plans
                    .iter()
                    .find(|(name, ..)| *name == setup)
                    .map(|(_, plan, _)| plan.hide)
            });
            let visibility = match (pad.setup.is_some(), pad.hidden, asked) {
                (false, ..) | (true, false, Some(Hide::None)) => "Visible",
                (true, true, _) => "Hidden",
                (true, false, _) => "Grabbed",
            };
            PadRow {
                is_virtual: false,
                identity: SharedString::default(),
                node: SharedString::default(),
                maker: SharedString::from(maker_of(pad.vendor)),
                word: SharedString::from(word),
                tone: SharedString::from(tone),
                visibility: SharedString::from(visibility),
                profiles: ModelRc::new(VecModel::from(named)),
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
                    // Free, with no rumble from its kernel driver; a
                    // setup rumbles it through its raw node.
                    Some(false) if pad.is_steam_controller() => "In a profile",
                    Some(false) => "No",
                    None => "",
                }),
                without_rumble: pad.rumble == Some(false),
                steam_controller: pad.is_steam_controller(),
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
pub fn show(app: &AppWindow, mut rows: Vec<PadRow>) {
    let pads = app.global::<Pads>();
    let current = pads.get_rows();
    // In place, down to each pad's profiles: see `models`.
    for row in &mut rows {
        if let Some(old) = current.iter().find(|old| old.id == row.id) {
            row.profiles =
                crate::models::sync(&old.profiles, row.profiles.iter().collect(), |a, b| {
                    a.name == b.name
                });
        }
    }
    let selected = pads
        .get_rows()
        .row_data(usize::try_from(pads.get_selected_index()).unwrap_or(usize::MAX))
        .map(|row| row.id);
    let index = selected
        .and_then(|id| rows.iter().position(|row| row.id == id))
        .and_then(|at| i32::try_from(at).ok())
        .unwrap_or(-1);
    let real = rows.iter().filter(|row| !row.is_virtual).count();
    let connected = rows
        .iter()
        .filter(|row| !row.is_virtual && row.connected)
        .count();
    pads.set_connected_count(i32::try_from(connected).unwrap_or(i32::MAX));
    pads.set_real_count(i32::try_from(real).unwrap_or(i32::MAX));
    // In place where the pads are the same ones: see `models`.
    pads.set_rows(crate::models::sync(&current, rows, |a, b| a.id == b.id));
    pads.set_selected_index(index);
}

/// The id of the pad selected, empty for none.
pub(crate) fn selected(app: &AppWindow) -> SharedString {
    let pads = app.global::<Pads>();
    pads.get_rows()
        .row_data(usize::try_from(pads.get_selected_index()).unwrap_or(usize::MAX))
        .map(|row| row.id)
        .unwrap_or_default()
}

/// Selects the pad of that id, or nothing when there is none.
pub(crate) fn select(app: &AppWindow, id: &str) {
    let pads = app.global::<Pads>();
    let index = pads
        .get_rows()
        .iter()
        .position(|row| row.id == id)
        .and_then(|at| i32::try_from(at).ok())
        .unwrap_or(-1);
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

/// Binds the link from a pad to its setup. It needs no daemon.
pub fn bind_links(app: &AppWindow, nav: &NavHandles) {
    let (weak, go_profile) = (app.as_weak(), nav.go_profile.clone());
    app.global::<Pads>().on_open_profile(move |profile| {
        if let Some(app) = weak.upgrade() {
            go_profile(&app, profile);
        }
    });
}

/// Binds Identify and Forget. What either changes comes back through
/// the mirror, as any change does.
pub(crate) fn wire(app: &AppWindow, daemon: &DaemonClient, nav: &NavHandles) {
    bind_links(app, nav);
    let pads = app.global::<Pads>();
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
