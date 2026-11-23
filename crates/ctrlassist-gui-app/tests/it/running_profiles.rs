//! The running profiles: a row for each of the daemon's setups, as the
//! Profiles pane's STATUS group shows it. What the group does with them
//! is tested with that pane.

use ctrlassist::{Phase, Setup, SetupNode, SetupSink, SetupSource};
use ctrlassist_gui::setups;
use slint::Model as _;

fn setup(name: &str, phase: Phase) -> Setup {
    Setup {
        name: name.to_string(),
        start: 1,
        phase,
        reason: (phase == Phase::Failed).then(|| "a virtual pad refused a write".to_string()),
        sources: vec![
            SetupSource {
                name: "assist".to_string(),
                pad: Some("address:1c:96:5a:a1:35:da".to_string()),
                present: phase != Phase::Held,
            },
            SetupSource {
                name: "primary".to_string(),
                pad: Some("address:0c:27:56:4f:bd:14".to_string()),
                present: true,
            },
        ],
        sinks: vec![SetupSink {
            name: "virtual".to_string(),
            identity: "dualsense".to_string(),
            node: Some("/dev/input/event261".to_string()),
        }],
        nodes: vec![SetupNode {
            name: "mux".to_string(),
            choices: vec!["primary".to_string(), "assist".to_string()],
            selected: 0,
        }],
        stale: false,
    }
}

#[test]
fn rows_say_whether_each_setup_s_profile_is_kept_and_its_selections() {
    let stale = Setup {
        stale: true,
        ..setup("couch", Phase::Running)
    };
    let orphan = Setup {
        stale: true,
        ..setup("attic", Phase::Held)
    };
    let all = [stale, orphan, setup("den", Phase::Failed)];
    let rows = setups::rows(&all, &["couch".to_string(), "den".to_string()]);
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["attic", "couch", "den"]);

    let couch = &rows[1];
    assert!(couch.stale && couch.profile_kept);
    let node = couch.nodes.row_data(0).unwrap();
    assert_eq!(node.selected, 0);
    assert_eq!(node.choices.row_count(), 2);

    let attic = &rows[0];
    assert!(attic.stale && !attic.profile_kept, "its profile is gone");

    // The badge is the worst of them.
    assert_eq!(setups::trouble(&all), "danger");
    assert_eq!(setups::trouble(&all[..2]), "warn");
    assert_eq!(setups::trouble(&[setup("couch", Phase::Running)]), "");
    assert_eq!(setups::trouble(&[]), "");
}
