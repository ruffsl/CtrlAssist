//! The profile schema: the architecture's examples as fixtures, and what
//! is said about a profile that is wrong.

#![expect(clippy::unwrap_used, reason = "tests fail loudly")]

mod wire;

use ctrlassist_core::{
    DemuxConfig, DemuxMode, Graph, GraphSpec, Input, Kind, Layout, Mode, MuxConfig, MuxFeedback,
    NodeId, NodeSpec, Rule, SourceId, SourceSpec, Switch, gamepad,
};
use ctrlassist_protocol::profile::{Identity, PadRef, Plan, Profile, ProfileError};

fn plan(text: &str) -> Result<Plan, ProfileError> {
    Profile::from_toml(text)?.plan()
}

/// Where a profile is wrong and what is said, for one that must be.
fn refusal(text: &str) -> (String, String) {
    let error = plan(text).unwrap_err();
    (error.at.clone(), error.problem)
}

fn source(deadzone: f32) -> SourceSpec {
    SourceSpec {
        deadzone,
        layout: Layout::gamepad(),
    }
}

const PRESET: &str = include_str!("fixtures/preset.toml");
const FULL: &str = include_str!("fixtures/full.toml");

#[test]
fn a_preset_is_two_pads_a_mux_and_a_virtual_pad() {
    let plan = plan(PRESET).unwrap();
    assert!(plan.auto_start);
    // Sources are numbered in name order.
    let names: Vec<&str> = plan
        .sources
        .iter()
        .map(|source| source.name.as_str())
        .collect();
    assert_eq!(names, ["assist", "primary"]);
    assert_eq!(plan.sources[0].pad, PadRef::Model(0x054c, 0x09cc));
    assert_eq!(
        plan.sources[1].pad,
        PadRef::Exact("address:0c:27:56:4f:bd:14".to_string())
    );
    assert_eq!(plan.nodes, ["mux"]);
    assert_eq!(plan.sinks.len(), 1);
    assert_eq!(
        (plan.sinks[0].name.as_str(), plan.sinks[0].identity),
        ("virtual", Identity::Generic)
    );

    let expected = GraphSpec {
        sources: vec![source(0.1), source(0.1)],
        nodes: vec![NodeSpec::Mux {
            // The primary first, the assist after it and so above it.
            inputs: vec![Input::Source(SourceId(1)), Input::Source(SourceId(0))],
            config: MuxConfig::new(Mode::Priority, &Layout::gamepad()),
        }],
        sinks: vec![Input::Node(NodeId(0), 0)],
    };
    assert_eq!(plan.graph, expected);
}

#[test]
fn the_full_form_is_the_graph_it_describes() {
    let plan = plan(FULL).unwrap();
    assert!(!plan.auto_start);
    let names: Vec<&str> = plan
        .sources
        .iter()
        .map(|source| source.name.as_str())
        .collect();
    assert_eq!(names, ["helper", "one", "two"]);
    assert_eq!(plan.nodes, ["first", "second", "share"]);
    let sinks: Vec<(&str, Identity)> = plan
        .sinks
        .iter()
        .map(|sink| (sink.name.as_str(), sink.identity))
        .collect();
    assert_eq!(
        sinks,
        [
            ("player-one", Identity::Generic),
            ("player-two", Identity::Xbox)
        ]
    );

    let (helper, one, two) = (SourceId(0), SourceId(1), SourceId(2));
    let (first, second, share) = (NodeId(0), NodeId(1), NodeId(2));
    let layout = Layout::gamepad();
    let mut tuned = MuxConfig::new(Mode::Priority, &layout);
    tuned.engage = 0.2;
    tuned.release = 0.1;
    for (index, (_, kind)) in layout.groups().iter().enumerate() {
        if *kind == Kind::Trigger {
            tuned.rules[index].1 = Rule::Blended;
        }
    }
    // Only the first of its inputs, `two`, feels this mux's feedback.
    tuned.feedback = MuxFeedback::Inputs(0b01);
    let expected = GraphSpec {
        sources: vec![source(0.15), source(0.1), source(0.1)],
        nodes: vec![
            NodeSpec::Mux {
                inputs: vec![Input::Source(one), Input::Node(share, 0)],
                config: MuxConfig::new(Mode::Priority, &layout),
            },
            NodeSpec::Mux {
                inputs: vec![Input::Source(two), Input::Node(share, 1)],
                config: tuned,
            },
            NodeSpec::Demux {
                input: Input::Source(helper),
                config: DemuxConfig::new(DemuxMode::Unicast, 2),
            },
        ],
        sinks: vec![Input::Node(first, 0), Input::Node(second, 0)],
    };
    assert_eq!(plan.graph, expected);
    // And the core agrees it is a graph.
    Graph::build(plan.graph).unwrap();
}

#[test]
fn every_preset_plans_to_a_graph_that_builds() {
    for preset in ["priority", "average", "toggle"] {
        let text = format!(
            "preset = \"{preset}\"\nprimary = \"model:045e:0b12\"\nassist = \"model:054c:0ce6\"\nidentity = \"xbox\"\n"
        );
        let plan = plan(&text).unwrap();
        assert_eq!(plan.sinks[0].identity, Identity::Xbox, "{preset}");
        Graph::build(plan.graph).unwrap();
    }
    for preset in ["unicast", "multicast"] {
        let plan = plan(&format!(
            "preset = \"{preset}\"\npad = \"model:045e:0b12\"\noutputs = 3\n"
        ))
        .unwrap();
        let names: Vec<&str> = plan.sinks.iter().map(|sink| sink.name.as_str()).collect();
        assert_eq!(names, ["virtual-0", "virtual-1", "virtual-2"], "{preset}");
        assert_eq!(
            plan.graph.sinks,
            [
                Input::Node(NodeId(0), 0),
                Input::Node(NodeId(0), 1),
                Input::Node(NodeId(0), 2)
            ]
        );
        Graph::build(plan.graph).unwrap();
    }
    // Two virtual pads unless told otherwise.
    assert_eq!(
        plan("preset = \"unicast\"\npad = \"model:045e:0b12\"\n")
            .unwrap()
            .sinks
            .len(),
        2
    );
}

#[test]
fn a_switch_is_a_button_by_name_on_any_input_or_one_or_none() {
    let node = |switch: &str| -> Result<Option<Switch>, ProfileError> {
        let text = format!(
            "[source.a]\npad = \"model:0001:0001\"\n[source.b]\npad = \"model:0002:0002\"\n\
             [node.m]\nmux = \"toggle\"\ninputs = [\"a\", \"b\"]\n{switch}\n[sink.out]\ninput = \"m\"\n"
        );
        match plan(&text)?.graph.nodes.remove(0) {
            NodeSpec::Mux { config, .. } => Ok(config.switch),
            NodeSpec::Demux { .. } => unreachable!("the node is a mux"),
        }
    };
    let on = |input, button| Some(Switch { input, button });
    assert_eq!(
        node(""),
        Ok(on(Some(1), gamepad::MODE)),
        "a toggle's own default"
    );
    assert_eq!(node("switch = \"select\""), Ok(on(None, gamepad::SELECT)));
    assert_eq!(
        node("switch = { button = \"start\", input = \"a\" }"),
        Ok(on(Some(0), gamepad::START))
    );
    assert_eq!(node("switch = false"), Ok(None));
    let error = node("switch = \"sideways\"").unwrap_err();
    assert_eq!(error.at, "node.m.switch");
    assert!(
        error.problem.contains("no button named `sideways`")
            && error.problem.contains("left-bumper"),
        "{error}"
    );
    assert_eq!(
        node("switch = { button = \"start\", input = \"c\" }")
            .unwrap_err()
            .at,
        "node.m.switch.input"
    );
    assert_eq!(node("switch = true").unwrap_err().at, "node.m.switch");
}

#[test]
fn a_pad_is_an_address_a_known_name_or_a_model() {
    let exact = |text: &str| Some(PadRef::Exact(text.to_string()));
    assert_eq!(
        PadRef::parse("0C:27:56:4F:BD:14"),
        exact("address:0c:27:56:4f:bd:14")
    );
    assert_eq!(
        PadRef::parse("address:0c:27:56:4f:bd:14"),
        exact("address:0c:27:56:4f:bd:14")
    );
    assert_eq!(
        PadRef::parse("serial:045e:02dd:7EED821FFB17"),
        exact("serial:045e:02dd:7EED821FFB17")
    );
    assert_eq!(
        PadRef::parse("port:045e:02dd:usb-0000:00:14.0-3/input0"),
        exact("port:045e:02dd:usb-0000:00:14.0-3/input0")
    );
    assert_eq!(
        PadRef::parse("model:1234:5678:Some Pad: Deluxe"),
        exact("model:1234:5678:Some Pad: Deluxe")
    );
    assert_eq!(
        PadRef::parse("model:054c:09cc"),
        Some(PadRef::Model(0x054c, 0x09cc))
    );
    for not_a_pad in [
        "",
        "dualshock4",
        "model:dualshock4",
        "model:54c:9cc",
        "address:nope",
        "serial:045e:02dd:",
        "serial:045e",
        "port:",
        "0c:27:56:4f:bd",
    ] {
        assert_eq!(PadRef::parse(not_a_pad), None, "{not_a_pad:?}");
    }

    let model = PadRef::Model(0x054c, 0x09cc);
    assert!(model.matches("address:aa:bb:cc:dd:ee:ff", 0x054c, 0x09cc));
    assert!(!model.matches("address:aa:bb:cc:dd:ee:ff", 0x054c, 0x0ce6));
    let one = PadRef::parse("0c:27:56:4f:bd:14").unwrap();
    assert!(one.matches("address:0c:27:56:4f:bd:14", 0, 0));
    assert!(!one.matches("address:0c:27:56:4f:bd:15", 0, 0));
}

#[test]
fn what_is_wrong_with_a_profile_is_said_with_where() {
    let base = "[source.a]\npad = \"model:0001:0001\"\n[source.b]\npad = \"model:0002:0002\"\n";
    let with = |rest: &str| refusal(&format!("{base}{rest}"));
    let said = |(at, problem): (String, String), want_at: &str, want: &str| {
        assert_eq!(at, want_at, "{problem}");
        assert!(problem.contains(want), "{at}: {problem}");
    };

    said(
        with("[node.m]\nmux = \"priority\"\ninputs = [\"a\", \"c\"]\n[sink.o]\ninput = \"m\"\n"),
        "node.m.inputs[1]",
        "no source or node named `c`",
    );
    said(
        with("[node.m]\nmux = \"priority\"\ninputs = [\"a\", \"b\"]\n[sink.o]\ninput = \"m.1\"\n"),
        "sink.o.input",
        "`m` is a mux, which has one output",
    );
    said(
        with(
            "[node.d]\ndemux = \"unicast\"\ninput = \"a\"\noutputs = 2\n[sink.o]\ninput = \"d.2\"\n",
        ),
        "sink.o.input",
        "`d` has 2 outputs, numbered from 0",
    );
    said(
        with("[sink.o]\ninput = \"a.0\"\n"),
        "sink.o.input",
        "`a` is a source",
    );
    said(
        with("[sink.o]\ninput = \"a.x\"\n"),
        "sink.o.input",
        "after the dot comes an output's number",
    );
    said(
        with("[node.m]\ninputs = [\"a\"]\n[sink.o]\ninput = \"m\"\n"),
        "node.m",
        "needs `mux` or `demux`",
    );
    said(
        with("[node.m]\nmux = \"priority\"\ndemux = \"unicast\"\n[sink.o]\ninput = \"m\"\n"),
        "node.m",
        "not both",
    );
    said(
        with("[node.m]\nmux = \"priority\"\n[sink.o]\ninput = \"m\"\n"),
        "node.m.inputs",
        "a mux needs inputs",
    );
    said(
        with(
            "[node.m]\nmux = \"priority\"\ninputs = [\"a\"]\noutputs = 2\n[sink.o]\ninput = \"m\"\n",
        ),
        "node.m.outputs",
        "belongs to a demux",
    );
    said(
        with(
            "[node.d]\ndemux = \"unicast\"\ninput = \"a\"\noutputs = 2\nengage = 0.5\n[sink.o]\ninput = \"d\"\n",
        ),
        "node.d.engage",
        "belongs to a mux",
    );
    said(
        with("[node.d]\ndemux = \"unicast\"\noutputs = 2\n[sink.o]\ninput = \"d\"\n"),
        "node.d.input",
        "needs an input",
    );
    said(
        with("[node.d]\ndemux = \"unicast\"\ninput = \"a\"\n[sink.o]\ninput = \"d\"\n"),
        "node.d.outputs",
        "how many outputs",
    );
    said(
        with(
            "[node.m]\nmux = \"priority\"\ninputs = [\"a\", \"b\"]\nfeedback = [\"c\"]\n[sink.o]\ninput = \"m\"\n",
        ),
        "node.m.feedback",
        "isn't one of this mux's inputs",
    );
    said(
        with("[node.a]\nmux = \"priority\"\ninputs = [\"b\"]\n[sink.o]\ninput = \"a\"\n"),
        "node.a",
        "is a source's name too",
    );
    said(
        with("[node.Bad_Name]\nmux = \"priority\"\ninputs = [\"a\"]\n[sink.o]\ninput = \"a\"\n"),
        "node.Bad_Name",
        "lowercase letters, digits and hyphens",
    );
    said(with(""), "sink", "at least one virtual pad");
    said(
        refusal("[sink.o]\ninput = \"a\"\n"),
        "sink.o.input",
        "no source or node named `a`",
    );
    said(
        refusal("[source.a]\npad = \"my controller\"\n[sink.o]\ninput = \"a\"\n"),
        "source.a.pad",
        "`my controller` isn't a pad",
    );

    // What the core refuses is passed on.
    said(
        with(
            "[node.m]\nmux = \"priority\"\ninputs = [\"a\", \"b\"]\nengage = 0.1\nrelease = 0.5\n[sink.o]\ninput = \"m\"\n",
        ),
        "",
        "the graph can't be built",
    );
    said(
        with(
            "[node.x]\nmux = \"priority\"\ninputs = [\"a\", \"y\"]\n[node.y]\nmux = \"priority\"\ninputs = [\"b\", \"x\"]\n[sink.o]\ninput = \"x\"\n",
        ),
        "",
        "the graph can't be built",
    );
}

#[test]
fn the_two_forms_do_not_mix_and_each_preset_has_its_own_keys() {
    let said = |text: &str, want_at: &str, want: &str| {
        let (at, problem) = refusal(text);
        assert_eq!(at, want_at, "{problem}");
        assert!(problem.contains(want), "{at}: {problem}");
    };
    said(
        "preset = \"priority\"\nprimary = \"model:0001:0001\"\n",
        "preset",
        "needs `primary` and `assist`",
    );
    said("preset = \"unicast\"\n", "preset", "needs `pad`");
    said(
        "preset = \"unicast\"\npad = \"model:0001:0001\"\nassist = \"model:0001:0001\"\n",
        "assist",
        "priority, average and toggle",
    );
    said(
        "preset = \"priority\"\nprimary = \"model:0001:0001\"\nassist = \"model:0001:0001\"\noutputs = 2\n",
        "outputs",
        "unicast and multicast",
    );
    said(
        "preset = \"unicast\"\npad = \"model:0001:0001\"\noutputs = 9\n",
        "outputs",
        "between 1 and 8",
    );
    said(
        "primary = \"model:0001:0001\"\n[sink.o]\ninput = \"a\"\n",
        "primary",
        "belongs to a preset",
    );
    said(
        "preset = \"priority\"\nprimary = \"model:0001:0001\"\nassist = \"model:0002:0002\"\n[source.x]\npad = \"model:0003:0003\"\n",
        "preset",
        "not both",
    );
    said(
        "preset = \"priority\"\nprimary = \"nonsense\"\nassist = \"model:0002:0002\"\n",
        "source.primary.pad",
        "isn't a pad",
    );
}

#[test]
fn text_that_is_not_a_profile_is_refused_with_its_line() {
    let refused = |text: &str| Profile::from_toml(text).unwrap_err();
    let error = refused("preset = \"priority\"\nprimry = \"x\"\n");
    assert_eq!(error.at, "line 2");
    assert!(error.problem.contains("primry"), "{error}");
    assert_eq!(refused("preset = \"sideways\"\n").at, "line 1");
    assert_eq!(refused("[source.a]\npad = 5\n").at, "line 2");
    assert_eq!(
        refused("[source.a]\npad = \"x\"\nwibble = 1\n").at,
        "line 3"
    );
    assert!(!refused("this is not toml").problem.is_empty());
    let error = refused("schema = 2\n");
    assert_eq!(
        error.to_string(),
        "schema: this profile is for schema 2, and this version reads up to 1"
    );
    assert_eq!(refused("schema = 0\n").at, "schema");
}

#[test]
fn a_profile_written_out_reads_back_the_same() {
    for text in [PRESET, FULL] {
        let profile = Profile::from_toml(text).unwrap();
        let written = profile.to_toml().unwrap();
        assert_eq!(Profile::from_toml(&written).unwrap(), profile, "{written}");
        // The schema version is left out while it is the current one.
        assert!(!written.contains("schema"), "{written}");
    }
    let older = Profile::from_toml("schema = 1\npreset = \"unicast\"\npad = \"model:0001:0001\"\n")
        .unwrap();
    assert_eq!(older.schema, 1);
}

#[test]
fn an_identity_goes_by_the_name_a_profile_writes() {
    for identity in Identity::ALL {
        assert_eq!(
            serde_json::to_value(identity).unwrap(),
            identity.as_str(),
            "{identity:?}"
        );
    }
}

#[test]
fn a_profile_s_name_is_letters_digits_and_hyphens() {
    use ctrlassist_protocol::profile::is_name;
    for name in ["couch", "player-2", "a", &"x".repeat(32)] {
        assert!(is_name(name), "{name}");
    }
    for name in [
        "",
        "Couch",
        "two words",
        "a/b",
        "../x",
        "a.toml",
        "*",
        &"x".repeat(33),
    ] {
        assert!(!is_name(name), "{name}");
    }
}

#[test]
fn checking_a_profile_reads_it_and_works_it_out() {
    let preset = include_str!("fixtures/preset.toml");
    assert_eq!(Profile::check(preset).unwrap().sinks.len(), 1);
    assert_eq!(
        Profile::check("preset = \"priority\"").unwrap_err().at,
        "preset"
    );
    assert_eq!(Profile::check("preset =").unwrap_err().at, "line 1");
}
