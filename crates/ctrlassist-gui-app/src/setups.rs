//! The running profiles: a row for each setup from the daemon's mirror,
//! which the Profiles pane's STATUS group shows, and what that group asks
//! of the daemon.

use ctrlassist::{DaemonClient, Phase, Setup};
use slint::{ComponentHandle, Model as _, ModelRc, SharedString, VecModel};
use tracing::warn;

use crate::{AppWindow, ErrorState, Errors, SetupNodeRow, SetupRow, Setups, on_ui};

/// The rows, in name order. `kept` is the names of the profiles kept
/// now.
#[must_use]
pub fn rows(setups: &[Setup], kept: &[String]) -> Vec<SetupRow> {
    let mut sorted: Vec<&Setup> = setups.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    sorted
        .into_iter()
        .map(|setup| {
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
                stale: setup.stale,
                profile_kept: kept.contains(&setup.name),
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
    // In place, down to each setup's nodes and a node's choices: see
    // `models`.
    let current = setups.get_rows();
    for row in &mut rows {
        let Some(old) = current.iter().find(|old| old.name == row.name) else {
            continue;
        };
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
                Ok(cost) => on_ui(&weak, move |app| {
                    let setups = app.global::<Setups>();
                    setups.set_pending_update_cost(SharedString::from(if cost.disruptive {
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
