//! The running profiles: a row for each setup from the daemon's mirror,
//! which the Profiles pane's STATUS group shows, and what that group asks
//! of the daemon.

use ctrlassist::{DaemonClient, Pad, Phase, Setup};
use slint::{ComponentHandle, Model as _, ModelRc, SharedString, VecModel};
use tracing::warn;

use crate::{
    AppWindow, ErrorState, Errors, SetupNodeRow, SetupPadRow, SetupRow, SetupSinkRow, Setups, on_ui,
};

/// A phase as a person reads it, and the chip's tone for it.
pub(crate) fn phase_text(phase: Phase) -> (&'static str, &'static str) {
    match phase {
        Phase::Running => ("Running", "success"),
        Phase::Held => ("Held: a pad is missing", "warn"),
        Phase::Failed => ("Failed", "danger"),
        // Starting, and any phase a later daemon adds.
        _ => ("Starting", "neutral"),
    }
}

/// The pane's rows, in name order. `pads` gives each pad its name;
/// `kept` is the names of the profiles kept now.
#[must_use]
pub fn rows(setups: &[Setup], pads: &[Pad], kept: &[String]) -> Vec<SetupRow> {
    let mut sorted: Vec<&Setup> = setups.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    sorted
        .into_iter()
        .map(|setup| {
            let (label, tone) = phase_text(setup.phase);
            let sources: Vec<SetupPadRow> = setup
                .sources
                .iter()
                .map(|source| {
                    let id = source.pad.clone().unwrap_or_default();
                    // A pad the daemon no longer knows goes by its id.
                    let name = pads
                        .iter()
                        .find(|pad| pad.id == id)
                        .map_or_else(|| id.clone(), |pad| pad.name.clone());
                    SetupPadRow {
                        role: SharedString::from(crate::status::capitalized(&source.name)),
                        pad: SharedString::from(id),
                        name: SharedString::from(name),
                        present: source.present,
                    }
                })
                .collect();
            let sinks: Vec<SetupSinkRow> = setup
                .sinks
                .iter()
                .map(|sink| SetupSinkRow {
                    name: SharedString::from(sink.name.as_str()),
                    identity: SharedString::from(sink.identity.as_str()),
                    node: SharedString::from(sink.node.clone().unwrap_or_default()),
                })
                .collect();
            let nodes: Vec<SetupNodeRow> = setup
                .nodes
                .iter()
                .map(|node| SetupNodeRow {
                    name: SharedString::from(node.name.as_str()),
                    choices: ModelRc::new(VecModel::from(
                        node.choices
                            .iter()
                            .map(|choice| SharedString::from(choice.as_str()))
                            .collect::<Vec<_>>(),
                    )),
                    selected: i32::try_from(node.selected).unwrap_or(-1),
                })
                .collect();
            SetupRow {
                name: SharedString::from(setup.name.as_str()),
                phase: SharedString::from(setup.phase.as_str()),
                phase_label: SharedString::from(label),
                tone: SharedString::from(tone),
                reason: SharedString::from(setup.reason.clone().unwrap_or_default()),
                stale: setup.stale,
                profile_kept: kept.contains(&setup.name),
                pads: ModelRc::new(VecModel::from(sources)),
                sinks: ModelRc::new(VecModel::from(sinks)),
                nodes: ModelRc::new(VecModel::from(nodes)),
            }
        })
        .collect()
}

/// The worst of what the setups are in, for the sidebar's badge.
#[must_use]
pub fn trouble(setups: &[Setup]) -> &'static str {
    if setups.iter().any(|setup| setup.phase == Phase::Failed) {
        "danger"
    } else if setups.iter().any(|setup| setup.phase == Phase::Held) {
        "warn"
    } else {
        ""
    }
}

/// Puts the rows on the window.
pub fn show(app: &AppWindow, mut rows: Vec<SetupRow>, trouble: &str) {
    let setups = app.global::<Setups>();
    // In place, down to each setup's pads, virtual pads, nodes and a
    // node's choices: see `models`.
    let current = setups.get_rows();
    for row in &mut rows {
        let Some(old) = current.iter().find(|old| old.name == row.name) else {
            continue;
        };
        row.pads = crate::models::sync(&old.pads, row.pads.iter().collect(), |a, b| {
            a.role == b.role
        });
        row.sinks = crate::models::sync(&old.sinks, row.sinks.iter().collect(), |a, b| {
            a.name == b.name
        });
        let mut nodes: Vec<SetupNodeRow> = row.nodes.iter().collect();
        for node in &mut nodes {
            if let Some(old) = old.nodes.iter().find(|old| old.name == node.name) {
                node.choices =
                    crate::models::sync(&old.choices, node.choices.iter().collect(), |a, b| a == b);
            }
        }
        row.nodes = crate::models::sync(&old.nodes, nodes, |a, b| a.name == b.name);
    }
    setups.set_trouble(SharedString::from(trouble));
    setups.set_rows(crate::models::sync(&current, rows, |a, b| a.name == b.name));
}

/// The pane's banner for something the daemon refused, or the empty
/// one that clears it.
fn banner(outcome: Result<(), (&str, ctrlassist::Error)>) -> ErrorState {
    match outcome {
        Ok(()) => ErrorState::default(),
        Err((what, error)) => {
            warn!(%error, "{what}");
            ErrorState {
                kind: SharedString::from("setups"),
                tone: SharedString::from("error"),
                title: SharedString::from(what),
                message: SharedString::from(error.to_string()),
                action_label: SharedString::default(),
            }
        }
    }
}

/// Binds the pane's actions. What each changes comes back through the
/// mirror, as any change does.
pub(crate) fn wire(app: &AppWindow, daemon: &DaemonClient) {
    let setups = app.global::<Setups>();

    let (weak, client) = (app.as_weak(), daemon.clone());
    setups.on_stop(move |name| {
        let (weak, client, name) = (weak.clone(), client.clone(), name.to_string());
        tokio::spawn(async move {
            let outcome = client
                .stop_setup(&name)
                .await
                .map_err(|error| ("The setup couldn't be stopped", error));
            let banner = banner(outcome);
            on_ui(&weak, move |app| app.global::<Errors>().set_setups(banner));
        });
    });

    // What an update would cost is asked first, and its dialog says.
    let (weak, client) = (app.as_weak(), daemon.clone());
    setups.on_ask_update(move |name| {
        let (weak, client, name) = (weak.clone(), client.clone(), name.to_string());
        tokio::spawn(async move {
            match client.update_setup(&name, false).await {
                Ok(disruptive) => on_ui(&weak, move |app| {
                    let setups = app.global::<Setups>();
                    setups.set_pending_update_cost(SharedString::from(if disruptive {
                        "restart"
                    } else {
                        "seamless"
                    }));
                    setups.set_pending_update(SharedString::from(name));
                }),
                Err(error) => {
                    let banner = banner(Err(("The setup can't be updated", error)));
                    on_ui(&weak, move |app| app.global::<Errors>().set_setups(banner));
                }
            }
        });
    });

    let (weak, client) = (app.as_weak(), daemon.clone());
    setups.on_update(move |name| {
        let (weak, client, name) = (weak.clone(), client.clone(), name.to_string());
        tokio::spawn(async move {
            let outcome = client
                .update_setup(&name, true)
                .await
                .map(|_| ())
                .map_err(|error| ("The setup couldn't be updated", error));
            let banner = banner(outcome);
            on_ui(&weak, move |app| app.global::<Errors>().set_setups(banner));
        });
    });

    let (weak, client) = (app.as_weak(), daemon.clone());
    setups.on_select(move |name, node, choice| {
        let (weak, client) = (weak.clone(), client.clone());
        let (name, node, choice) = (name.to_string(), node.to_string(), choice.to_string());
        tokio::spawn(async move {
            let outcome = client
                .select(&name, &node, Some(&choice))
                .await
                .map_err(|error| ("Control couldn't be handed over", error));
            let banner = banner(outcome);
            on_ui(&weak, move |app| app.global::<Errors>().set_setups(banner));
        });
    });
}
