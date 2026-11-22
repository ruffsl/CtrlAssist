//! The running profiles: a row for each of the daemon's setups, as the
//! Profiles pane's STATUS group shows it. What the group does with them
//! is tested with that pane.

use ctrlassist::{Pad, Phase, Setup, SetupNode, SetupSink, SetupSource};
use ctrlassist_gui::setups;
use slint::Model as _;

fn pad(id: &str, name: &str, setup: Option<&str>) -> Pad {
    Pad {
        id: id.to_string(),
        name: name.to_string(),
        vendor: 0x054c,
        product: 0x0ce6,
        connected: true,
        hidden: setup.is_some(),
        setup: setup.map(str::to_string),
        rumble: Some(true),
        bus: Some(3),
    }
}

fn setup(name: &str, phase: Phase) -> Setup {
    Setup {
        name: name.to_string(),
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

fn some_pads() -> Vec<Pad> {
    vec![
        pad("address:0c:27:56:4f:bd:14", "DualSense", Some("couch")),
        pad(
            "address:1c:96:5a:a1:35:da",
            "Wireless Controller",
            Some("couch"),
        ),
    ]
}

#[test]
fn rows_say_each_setup_s_phase_pads_virtual_pads_and_selections() {
    let stale = Setup {
        stale: true,
        ..setup("couch", Phase::Running)
    };
    let orphan = Setup {
        stale: true,
        // A pad the daemon no longer knows, and one never found.
        sources: vec![
            SetupSource {
                name: "primary".to_string(),
                pad: Some("serial:gone".to_string()),
                present: false,
            },
            SetupSource {
                name: "assist".to_string(),
                pad: None,
                present: false,
            },
        ],
        ..setup("attic", Phase::Held)
    };
    let all = [stale, orphan, setup("den", Phase::Failed)];
    let rows = setups::rows(
        &all,
        &some_pads(),
        &["couch".to_string(), "den".to_string()],
    );
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["attic", "couch", "den"]);

    let couch = &rows[1];
    assert_eq!(
        (couch.phase.as_str(), couch.tone.as_str()),
        ("running", "success")
    );
    assert!(couch.stale && couch.profile_kept);
    let pads: Vec<_> = couch.pads.iter().collect();
    assert_eq!(pads[0].role, "Assist");
    assert_eq!(pads[0].name, "Wireless Controller");
    assert!(pads[0].present);
    let sink = couch.sinks.row_data(0).unwrap();
    assert_eq!(
        (sink.identity.as_str(), sink.node.as_str()),
        ("dualsense", "/dev/input/event261")
    );
    let node = couch.nodes.row_data(0).unwrap();
    assert_eq!(node.selected, 0);
    assert_eq!(node.choices.row_count(), 2);

    let attic = &rows[0];
    assert_eq!(attic.tone, "warn");
    assert!(attic.stale && !attic.profile_kept, "its profile is gone");
    let pads: Vec<_> = attic.pads.iter().collect();
    assert_eq!(
        (pads[0].pad.as_str(), pads[0].name.as_str()),
        ("serial:gone", "serial:gone")
    );
    assert_eq!(pads[1].pad, "");

    let den = &rows[2];
    assert_eq!(den.tone, "danger");
    assert_eq!(den.reason, "a virtual pad refused a write");

    // The badge is the worst of them.
    assert_eq!(setups::trouble(&all), "danger");
    assert_eq!(setups::trouble(&all[..2]), "warn");
    assert_eq!(setups::trouble(&[setup("couch", Phase::Running)]), "");
    assert_eq!(setups::trouble(&[]), "");
}
