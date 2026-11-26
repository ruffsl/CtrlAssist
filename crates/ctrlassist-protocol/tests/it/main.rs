//! The profile schema: the architecture's examples as fixtures, and what
//! is said about a profile that is wrong.

#![expect(clippy::unwrap_used, reason = "tests fail loudly")]

mod wire;

use ctrlassist_core::{
    DemuxConfig, DemuxMode, Graph, GraphSpec, Input, Kind, Layout, Mode, MuxConfig, MuxFeedback,
    NodeId, NodeSpec, Rule, SourceId, SourceSpec, Switch, gamepad,
};
use ctrlassist_protocol::profile::{
    Hide, Identity, PadRef, Plan, Profile, ProfileError, button_names,
};

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
    assert!(plan.managed);
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
        PadRef::Exact("address:0a:1b:2c:3d:4e:14".to_string())
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
    assert!(!plan.managed);
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
            "mode = \"{preset}\"\nprimary = \"model:045e:0b12\"\nassist = \"model:054c:0ce6\"\nidentity = \"xbox\"\n"
        );
        let plan = plan(&text).unwrap();
        assert_eq!(plan.sinks[0].identity, Identity::Xbox, "{preset}");
        Graph::build(plan.graph).unwrap();
    }
    for preset in ["unicast", "multicast"] {
        let plan = plan(&format!(
            "mode = \"{preset}\"\npad = \"model:045e:0b12\"\noutputs = 3\n"
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
        plan("mode = \"unicast\"\npad = \"model:045e:0b12\"\n")
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
    assert_eq!(
        node("switch = { button = \"paddle-4\", input = \"b\" }"),
        Ok(on(Some(1), gamepad::PADDLE_4)),
        "a back button"
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
        PadRef::parse("0A:1B:2C:3D:4E:14"),
        exact("address:0a:1b:2c:3d:4e:14")
    );
    assert_eq!(
        PadRef::parse("address:0a:1b:2c:3d:4e:14"),
        exact("address:0a:1b:2c:3d:4e:14")
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
    // The vendor and product in capitals are the numbers the device
    // crate writes in lowercase; the serial, port or name is kept.
    assert_eq!(
        PadRef::parse("serial:054C:09CC:7EED821FFB17"),
        exact("serial:054c:09cc:7EED821FFB17")
    );
    assert_eq!(
        PadRef::parse("port:045E:02DD:usb-0000:00:14.0-3/input0"),
        exact("port:045e:02dd:usb-0000:00:14.0-3/input0")
    );
    assert_eq!(
        PadRef::parse("model:1A34:5B78:Some Pad: Deluxe"),
        exact("model:1a34:5b78:Some Pad: Deluxe")
    );
    assert_eq!(
        PadRef::parse("model:054C:09CC"),
        Some(PadRef::Model(0x054c, 0x09cc))
    );
    // An address that is no MAC address, as a Steam Controller's serial
    // number is, is kept as written.
    assert_eq!(
        PadRef::parse("address:FX12AB34CD"),
        exact("address:FX12AB34CD")
    );
    let written = PadRef::parse("serial:054C:09CC:abc").unwrap();
    assert!(written.matches("serial:054c:09cc:abc", 0x054c, 0x09cc));
    assert!(!written.matches("serial:054c:09cc:ABC", 0x054c, 0x09cc));
    for not_a_pad in [
        "model:+54c:09cc",
        "serial:054c:-9cc:abc",
        "",
        "dualshock4",
        "model:dualshock4",
        "model:54c:9cc",
        "address:no/pe",
        "address:0a:1b:2c",
        "serial:045e:02dd:",
        "serial:045e",
        "port:",
        "0a:1b:2c:3d:4e",
    ] {
        assert_eq!(PadRef::parse(not_a_pad), None, "{not_a_pad:?}");
    }

    let model = PadRef::Model(0x054c, 0x09cc);
    assert!(model.matches("address:aa:bb:cc:dd:ee:ff", 0x054c, 0x09cc));
    assert!(!model.matches("address:aa:bb:cc:dd:ee:ff", 0x054c, 0x0ce6));
    let one = PadRef::parse("0a:1b:2c:3d:4e:14").unwrap();
    assert!(one.matches("address:0a:1b:2c:3d:4e:14", 0, 0));
    assert!(!one.matches("address:0a:1b:2c:3d:4e:15", 0, 0));
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
            "[node.x]\nmux = \"priority\"\ninputs = [\"a\", \"y\"]\n[node.y]\nmux = \"priority\"\ninputs = [\"b\", \"x\"]\n[sink.o]\ninput = \"x\"\n",
        ),
        "",
        "the graph can't be built",
    );
}

/// What the core would refuse of a node or a source is said with where
/// it is, even where it is a default the profile didn't write.
#[test]
fn a_refusal_the_core_would_make_is_said_with_where() {
    let base = "[source.a]\npad = \"model:0001:0001\"\n[source.b]\npad = \"model:0002:0002\"\n";
    let mux = |rest: &str| {
        format!(
            "{base}[node.m]\nmux = \"priority\"\ninputs = [\"a\", \"b\"]\n{rest}[sink.o]\ninput = \"m\"\n"
        )
    };
    let demux = |outputs: usize| {
        format!(
            "{base}[node.d]\ndemux = \"unicast\"\ninput = \"a\"\noutputs = {outputs}\n[sink.o]\ninput = \"d\"\n"
        )
    };
    for (text, want_at, want) in [
        // Never reached, so a group never changes hands.
        (
            mux("engage = 1.5\nrelease = 0.5\n"),
            "node.m.engage",
            "more than 0 and at most 1",
        ),
        (mux("engage = nan\n"), "node.m.engage", "at most 1"),
        // Never gone below, so a group never changes hands back.
        (mux("release = 0.0\n"), "node.m.release", "never let go"),
        (
            mux("engage = 0.1\nrelease = 0.5\n"),
            "node.m.release",
            "`release` (0.5) is at most `engage` (0.1)",
        ),
        // Only one written: it is that one that is wrong.
        (
            mux("release = 0.2\n"),
            "node.m.release",
            "`release` (0.2) is at most `engage` (0.1)",
        ),
        (
            mux("engage = 0.03\n"),
            "node.m.engage",
            "`release` (0.05) is at most `engage` (0.03)",
        ),
        (
            format!(
                "{base}[node.m]\nmux = \"priority\"\ninputs = [\"a\", \"b\", \"a\", \"b\", \"a\", \"b\", \"a\", \"b\", \"a\"]\n[sink.o]\ninput = \"m\"\n"
            ),
            "node.m.inputs",
            "at most 8 inputs",
        ),
        (demux(0), "node.d.outputs", "between 1 and 16"),
        (demux(17), "node.d.outputs", "between 1 and 16"),
        (
            "[source.a]\npad = \"model:0001:0001\"\ndeadzone = -0.1\n[sink.o]\ninput = \"a\"\n"
                .to_string(),
            "source.a.deadzone",
            "a number from 0",
        ),
        (
            "[source.a]\npad = \"model:0001:0001\"\ndeadzone = 1.0\n[sink.o]\ninput = \"a\"\n"
                .to_string(),
            "source.a.deadzone",
            "less than 1",
        ),
        (
            "[source.a]\npad = \"model:0001:0001\"\ndeadzone = 2.5\n[sink.o]\ninput = \"a\"\n"
                .to_string(),
            "source.a.deadzone",
            "less than 1",
        ),
    ] {
        let (at, problem) = refusal(&text);
        assert_eq!(at, want_at, "{problem}\n{text}");
        assert!(problem.contains(want), "{at}: {problem}");
    }
    plan(&mux("engage = 1.0\nrelease = 1.0\n")).unwrap();
    plan(&demux(16)).unwrap();
    // A deadzone from 0 to just short of 1 leaves some travel; none
    // written is the default, which does too.
    for deadzone in ["deadzone = 0.0\n", "deadzone = 0.99\n", ""] {
        let text =
            format!("[source.a]\npad = \"model:0001:0001\"\n{deadzone}[sink.o]\ninput = \"a\"\n");
        let deadzones: Vec<f32> = plan(&text)
            .unwrap()
            .graph
            .sources
            .iter()
            .map(|source| source.deadzone)
            .collect();
        assert!(
            deadzones
                .iter()
                .all(|deadzone| (0.0..1.0).contains(deadzone)),
            "{deadzones:?}"
        );
    }

    // A toggle of one input switches on that input's button, not on a
    // second input it doesn't have.
    let one =
        format!("{base}[node.m]\nmux = \"toggle\"\ninputs = [\"a\"]\n[sink.o]\ninput = \"m\"\n");
    match plan(&one).unwrap().graph.nodes.remove(0) {
        NodeSpec::Mux { config, .. } => assert_eq!(
            config.switch,
            Some(Switch {
                input: Some(0),
                button: gamepad::MODE
            })
        ),
        NodeSpec::Demux { .. } => unreachable!("the node is a mux"),
    }
}

/// A switch is only for a node that selects: on one that doesn't, it
/// would only keep its button from the game.
#[test]
fn a_switch_is_for_a_node_that_selects() {
    let base = "[source.a]\npad = \"model:0001:0001\"\n[source.b]\npad = \"model:0002:0002\"\n";
    let mux = |mode: &str, rest: &str| {
        format!(
            "{base}[node.m]\nmux = \"{mode}\"\ninputs = [\"a\", \"b\"]\n{rest}[sink.o]\ninput = \"m\"\n"
        )
    };
    let switch_of = |text: &str| match plan(text).unwrap().graph.nodes.remove(0) {
        NodeSpec::Mux { config, .. } => config.switch,
        NodeSpec::Demux { config, .. } => config.switch.map(|button| Switch {
            input: None,
            button,
        }),
    };
    for (text, at) in [
        (mux("priority", "switch = \"select\"\n"), "node.m.switch"),
        (
            mux(
                "average",
                "switch = { button = \"select\", input = \"b\" }\n",
            ),
            "node.m.switch",
        ),
        // A toggle whose rules leave nothing switched, with a switch.
        (
            mux(
                "toggle",
                "switch = \"select\"\nrules = { stick = \"owned\", trigger = \"owned\", button = \"owned\", hat = \"owned\" }\n",
            ),
            "node.m.switch",
        ),
        (
            format!(
                "{base}[node.d]\ndemux = \"multicast\"\ninput = \"a\"\noutputs = 2\nswitch = \"select\"\n[sink.o]\ninput = \"d\"\n"
            ),
            "node.d.switch",
        ),
    ] {
        let (where_, problem) = refusal(&text);
        assert_eq!(where_, at, "{problem}\n{text}");
        assert!(problem.contains("never selects"), "{problem}");
    }
    // One switched rule makes a switch useful.
    assert_eq!(
        switch_of(&mux(
            "priority",
            "switch = \"select\"\nrules = { button = \"switched\" }\n"
        )),
        Some(Switch {
            input: None,
            button: gamepad::SELECT
        })
    );
    // A toggle whose rules leave nothing switched doesn't take the
    // toggle's own switch, and a priority mux has none to take.
    assert_eq!(
        switch_of(&mux(
            "toggle",
            "rules = { stick = \"owned\", trigger = \"owned\", button = \"owned\", hat = \"owned\" }\n"
        )),
        None
    );
    assert_eq!(switch_of(&mux("priority", "")), None);
    // A multicast has no switch of its own, and `false` says as much.
    assert_eq!(
        switch_of(&format!(
            "{base}[node.d]\ndemux = \"multicast\"\ninput = \"a\"\noutputs = 2\nswitch = false\n[sink.o]\ninput = \"d\"\n"
        )),
        None
    );
}

/// A profile of thousands of virtual pads fits in a file the daemon
/// takes; it is refused, saying how many, before anything is made.
#[test]
fn a_profile_has_at_most_32_pads_64_nodes_and_32_virtual_pads() {
    let sources = |count: usize| -> String {
        (0..count)
            .map(|at| format!("[source.p{at}]\npad = \"model:0001:{:04x}\"\n", at + 1))
            .collect()
    };
    let sinks = |count: usize| -> String {
        (0..count)
            .map(|at| format!("[sink.v{at}]\ninput = \"p0\"\n"))
            .collect()
    };
    let nodes = |count: usize| -> String {
        (0..count)
            .map(|at| format!("[node.n{at}]\nmux = \"priority\"\ninputs = [\"p0\"]\n"))
            .collect()
    };
    plan(&format!("{}{}{}", sources(32), nodes(64), sinks(32))).unwrap();
    for (text, at, said) in [
        (
            format!("{}{}", sources(33), sinks(1)),
            "source",
            "at most 32 pads, and this one has 33",
        ),
        (
            format!("{}{}{}", sources(1), nodes(65), sinks(1)),
            "node",
            "at most 64 nodes",
        ),
        (
            format!("{}{}", sources(1), sinks(2500)),
            "sink",
            "at most 32 virtual pads, and this one has 2500",
        ),
    ] {
        let (where_, problem) = refusal(&text);
        assert_eq!(where_, at, "{problem}");
        assert!(problem.contains(said), "{problem}");
    }
}

/// The one mux of a profile's plan, as configured.
fn mux_of(text: &str) -> MuxConfig {
    let nodes = plan(text).unwrap().graph.nodes;
    let mut muxes = nodes.into_iter().filter_map(|node| match node {
        NodeSpec::Mux { config, .. } => Some(config),
        NodeSpec::Demux { .. } => None,
    });
    muxes.next().unwrap()
}

#[test]
fn a_preset_names_the_assists_button_that_takes_the_motion() {
    let preset = |mode: &str, key: &str| {
        format!(
            "mode = \"{mode}\"\nprimary = \"model:0001:0001\"\nassist = \"model:0001:0002\"\n{key}"
        )
    };
    // The assist is the mux's second input.
    for mode in ["priority", "average"] {
        assert_eq!(
            mux_of(&preset(mode, "assist-gyro = \"left-bumper\"\n")).gyro,
            Some(Switch {
                input: Some(1),
                button: gamepad::LEFT_BUMPER,
            }),
            "{mode}"
        );
        assert_eq!(mux_of(&preset(mode, "")).gyro, None);
        assert_eq!(mux_of(&preset(mode, "assist-gyro = \"none\"\n")).gyro, None);
    }
    // The touchpad's click is a button with a name.
    assert_eq!(
        mux_of(&preset("priority", "assist-gyro = \"touchpad\"\n"))
            .gyro
            .map(|gyro| gyro.button),
        Some(gamepad::TOUCHPAD)
    );
    // So are the back buttons, in SDL's order, and capacitive touch is
    // not.
    for (name, paddle) in [
        ("paddle-1", gamepad::PADDLE_1),
        ("paddle-2", gamepad::PADDLE_2),
        ("paddle-3", gamepad::PADDLE_3),
        ("paddle-4", gamepad::PADDLE_4),
    ] {
        assert_eq!(
            mux_of(&preset("average", &format!("assist-gyro = \"{name}\"\n")))
                .gyro
                .map(|gyro| gyro.button),
            Some(paddle),
            "{name}"
        );
    }
    assert!(
        !button_names().any(|name| name.contains("touch") && name != "touchpad"),
        "{:?}",
        button_names().collect::<Vec<_>>()
    );

    let said = |text: &str, want_at: &str, want: &str| {
        let (at, problem) = refusal(text);
        assert_eq!(at, want_at, "{problem}");
        assert!(problem.contains(want), "{at}: {problem}");
    };
    said(
        &preset("priority", "assist-gyro = \"left-x\"\n"),
        "assist-gyro",
        "no button named `left-x`",
    );
    said(
        &preset("toggle", "assist-gyro = \"south\"\n"),
        "assist-gyro",
        "follow the pad that has control",
    );
    assert_eq!(
        mux_of(&preset("toggle", "assist-gyro = \"none\"\n")).gyro,
        None
    );
    said(
        "mode = \"unicast\"\npad = \"model:0001:0001\"\nassist-gyro = \"south\"\n",
        "assist-gyro",
        "priority and average",
    );
    said(
        "assist-gyro = \"south\"\n",
        "assist-gyro",
        "belongs to a preset",
    );
}

#[test]
fn the_full_form_names_a_muxs_button_that_hands_motion_over() {
    let full = |mode: &str, gyro: &str| {
        format!(
            "[source.a]\npad = \"model:0001:0001\"\n[source.b]\npad = \"model:0001:0002\"\n[node.m]\nmux = \"{mode}\"\ninputs = [\"a\", \"b\"]\n{gyro}[sink.o]\ninput = \"m\"\nidentity = \"dualsense\"\n"
        )
    };
    assert_eq!(
        mux_of(&full(
            "average",
            "gyro = { button = \"south\", input = \"a\" }\n"
        ))
        .gyro,
        Some(Switch {
            input: Some(0),
            button: gamepad::SOUTH,
        })
    );
    assert_eq!(
        mux_of(&full(
            "priority",
            "gyro = { button = \"paddle-2\", input = \"b\" }\n"
        ))
        .gyro,
        Some(Switch {
            input: Some(1),
            button: gamepad::PADDLE_2,
        }),
        "a back button"
    );
    let said = |text: &str, want_at: &str, want: &str| {
        let (at, problem) = refusal(text);
        assert_eq!(at, want_at, "{problem}");
        assert!(problem.contains(want), "{at}: {problem}");
    };
    said(
        &full("priority", "gyro = { button = \"south\", input = \"c\" }\n"),
        "node.m.gyro.input",
        "isn't one of this mux's inputs",
    );
    said(
        &full("priority", "gyro = { button = \"souht\", input = \"b\" }\n"),
        "node.m.gyro.button",
        "no button named",
    );
    said(
        &full("toggle", "gyro = { button = \"south\", input = \"b\" }\n"),
        "node.m.gyro",
        "follow the input selected",
    );
    said(
        "[source.a]\npad = \"model:0001:0001\"\n[node.d]\ndemux = \"unicast\"\ninput = \"a\"\noutputs = 2\ngyro = { button = \"south\", input = \"a\" }\n[sink.o]\ninput = \"d.0\"\nidentity = \"generic\"\n",
        "node.d.gyro",
        "belongs to a mux",
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
        "mode = \"priority\"\nprimary = \"model:0001:0001\"\n",
        "mode",
        "needs `primary` and `assist`",
    );
    said("mode = \"unicast\"\n", "mode", "needs `pad`");
    said(
        "mode = \"unicast\"\npad = \"model:0001:0001\"\nassist = \"model:0001:0001\"\n",
        "assist",
        "priority, average and toggle",
    );
    said(
        "mode = \"priority\"\nprimary = \"model:0001:0001\"\nassist = \"model:0001:0001\"\noutputs = 2\n",
        "outputs",
        "unicast and multicast",
    );
    said(
        "mode = \"unicast\"\npad = \"model:0001:0001\"\noutputs = 9\n",
        "outputs",
        "between 1 and 8",
    );
    said(
        "primary = \"model:0001:0001\"\n[sink.o]\ninput = \"a\"\n",
        "primary",
        "belongs to a preset",
    );
    said(
        "mode = \"priority\"\nprimary = \"model:0001:0001\"\nassist = \"model:0002:0002\"\n[source.x]\npad = \"model:0003:0003\"\n",
        "mode",
        "not both",
    );
    said(
        "mode = \"priority\"\nprimary = \"nonsense\"\nassist = \"model:0002:0002\"\n",
        "source.primary.pad",
        "isn't a pad",
    );
}

#[test]
fn text_that_is_not_a_profile_is_refused_with_its_line() {
    let refused = |text: &str| Profile::from_toml(text).unwrap_err();
    let error = refused("mode = \"priority\"\nprimry = \"x\"\n");
    assert_eq!(error.at, "line 2");
    assert!(error.problem.contains("primry"), "{error}");
    assert_eq!(refused("mode = \"sideways\"\n").at, "line 1");
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
    // A newer schema is said to be one even where the file has keys this
    // version doesn't know, as a newer file may well have, wherever the
    // schema is written.
    for newer in [
        "schema = 2\nmode = \"priority\"\nfuture = true\n",
        "mode = \"priority\"\n[node.m]\nnew-key = 1\n",
    ] {
        let text = if newer.starts_with("schema") {
            newer.to_string()
        } else {
            format!("schema = 2\n{newer}")
        };
        assert_eq!(
            refused(&text).to_string(),
            "schema: this profile is for schema 2, and this version reads up to 1",
            "{text}"
        );
    }
}

#[test]
fn a_profile_may_say_its_schema_and_is_the_current_one_if_not() {
    let said =
        Profile::from_toml("schema = 1\nmode = \"unicast\"\npad = \"model:0001:0001\"\n").unwrap();
    assert_eq!(said.schema, 1);
    assert_eq!(Profile::from_toml(PRESET).unwrap().schema, 1);
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
        Profile::check("mode = \"priority\"").unwrap_err().at,
        "mode"
    );
    assert_eq!(Profile::check("preset =").unwrap_err().at, "line 1");
}

/// Any profile may say how its pads are kept from games, a preset or
/// the full form, and one that doesn't means as well as can be.
#[test]
fn a_profile_says_how_its_pads_are_hidden() {
    let preset = |hide: &str| {
        format!(
            "mode = \"priority\"\nprimary = \"model:0001:0001\"\nassist = \"model:0002:0002\"\n{hide}"
        )
    };
    assert_eq!(plan(&preset("")).unwrap().hide, Hide::Full);
    for hide in Hide::ALL {
        let said = format!("hide = \"{}\"\n", hide.as_str());
        assert_eq!(plan(&preset(&said)).unwrap().hide, hide);
        assert_eq!(plan(&format!("{said}{FULL}")).unwrap().hide, hide);
    }
    assert_eq!(plan(FULL).unwrap().hide, Hide::Full);
    assert!(plan(&preset("hide = \"somewhat\"\n")).is_err());
}

/// A preset of two pads may say who feels rumble, and means what the
/// full form means by a mux's `feedback`.
#[test]
fn a_preset_s_rumble_is_the_mux_s_feedback() {
    let preset = |rumble: &str| {
        format!(
            "mode = \"toggle\"\nprimary = \"model:0001:0001\"\nassist = \"model:0002:0002\"\n{rumble}"
        )
    };
    let full = |feedback: &str| {
        format!(
            "[source.primary]\npad = \"model:0001:0001\"\n[source.assist]\npad = \"model:0002:0002\"\n[node.mux]\nmux = \"toggle\"\ninputs = [\"primary\", \"assist\"]\n{feedback}[sink.virtual]\ninput = \"mux\"\n"
        )
    };
    for (rumble, feedback) in [
        ("", ""),
        ("rumble = \"both\"\n", ""),
        ("rumble = \"primary\"\n", "feedback = [\"primary\"]\n"),
        ("rumble = \"assist\"\n", "feedback = [\"assist\"]\n"),
        ("rumble = \"none\"\n", "feedback = \"none\"\n"),
    ] {
        assert_eq!(
            plan(&preset(rumble)).unwrap().graph,
            plan(&full(feedback)).unwrap().graph,
            "{rumble}"
        );
    }
    // The two that differ really do.
    assert_ne!(
        plan(&preset("rumble = \"primary\"\n")).unwrap().graph,
        plan(&preset("rumble = \"assist\"\n")).unwrap().graph
    );

    // It belongs to those presets alone.
    let (at, problem) =
        refusal("mode = \"unicast\"\npad = \"model:0001:0001\"\nrumble = \"primary\"\n");
    assert_eq!(at, "rumble");
    assert!(
        problem.contains("priority, average and toggle"),
        "{problem}"
    );
    let (at, problem) = refusal("rumble = \"both\"\n[sink.o]\ninput = \"a\"\n");
    assert_eq!(at, "rumble");
    assert!(problem.contains("belongs to a preset"), "{problem}");
    let (_, problem) = refusal(&preset("rumble = \"everyone\"\n"));
    assert!(
        problem.contains("rumble") || problem.contains("everyone"),
        "{problem}"
    );
}

mod form {
    //! The described fields against the profiles they stand for.

    use ctrlassist_protocol::form::{self, Kind, Values};
    use ctrlassist_protocol::profile::Profile;

    /// A value the field could hold, and another.
    fn samples(field: &form::Field) -> Vec<String> {
        match &field.kind {
            Kind::Choice { choices } => choices.iter().map(|choice| choice.value.clone()).collect(),
            Kind::Pad => vec![format!("model:0001:{:04x}", field.key.len())],
            Kind::Number { min, max } => vec![min.to_string(), max.to_string()],
            Kind::Switch => vec!["false".to_string(), "true".to_string()],
            _ => unreachable!("a kind the test doesn't know"),
        }
    }

    /// Every preset, with every other field at each of its values in
    /// turn, writes a profile the daemon would keep, and reads back.
    #[test]
    fn what_a_form_writes_is_a_profile_and_reads_back_as_written() {
        let fields = form::fields();
        let presets = samples(&fields[0]);
        assert_eq!(presets.len(), 5);
        let mut made = 0;
        for preset in &presets {
            // A filled form, every field at its first value.
            let mut base = Values::new();
            for field in &fields {
                base.insert(field.key.clone(), samples(field)[0].clone());
            }
            base.insert(form::PRESET.to_string(), preset.clone());
            for field in fields.iter().skip(1) {
                for value in samples(field) {
                    let mut values = base.clone();
                    values.insert(field.key.clone(), value);
                    let toml = form::to_toml(&fields, &values);
                    Profile::check(&toml)
                        .unwrap_or_else(|problem| unreachable!("{problem}\n{toml}"));
                    made += 1;
                    // Read back: every field of this preset, at what was
                    // set, and none that isn't its own.
                    let read = form::from_toml(&fields, &toml).unwrap();
                    for field in &fields {
                        if field.applies_to(preset) {
                            assert_eq!(read.get(&field.key), values.get(&field.key), "{toml}");
                        } else {
                            assert_eq!(read.get(&field.key), None, "{toml}");
                        }
                    }
                }
            }
        }
        assert!(made > 50, "{made}");
    }

    #[test]
    fn a_default_is_left_out_and_read_back_in() {
        let fields = form::fields();
        let mut values = Values::new();
        for (key, value) in [
            ("mode", "toggle"),
            ("primary", "0a:1b:2c:3d:4e:14"),
            ("assist", "model:054c:09cc"),
            ("identity", "generic"),
            ("rumble", "both"),
            ("managed", "false"),
            // Not this preset's, and so not written.
            ("outputs", "4"),
        ] {
            values.insert(key.to_string(), value.to_string());
        }
        let toml = form::to_toml(&fields, &values);
        assert_eq!(
            toml,
            "mode = \"toggle\"\nprimary = \"0a:1b:2c:3d:4e:14\"\nassist = \"model:054c:09cc\"\n"
        );
        let read = form::from_toml(&fields, &toml).unwrap();
        assert_eq!(read["identity"], "generic");
        assert_eq!(read["rumble"], "both");
        assert_eq!(read["managed"], "false");
        assert_eq!(read.get("outputs"), None);
    }

    /// A value that isn't what its field holds is written as a string,
    /// so it can add nothing to the profile and the check refuses it.
    #[test]
    fn a_value_can_add_nothing_to_what_a_form_writes() {
        let fields = form::fields();
        let mut values = Values::new();
        for (key, value) in [
            ("mode", "unicast"),
            ("pad", "model:0001:0001"),
            ("outputs", "3\n[source.x]\npad = \"model:0002:0002\""),
            ("managed", "yes\nprimary = \"x\""),
        ] {
            values.insert(key.to_string(), value.to_string());
        }
        let toml = form::to_toml(&fields, &values);
        let table: toml::Table = toml.parse().unwrap();
        let keys: Vec<&str> = table.keys().map(String::as_str).collect();
        assert_eq!(keys, ["managed", "mode", "outputs", "pad"], "{toml}");
        assert!(Profile::check(&toml).is_err(), "{toml}");
        // A number is written as one, however it came.
        values.insert("outputs".to_string(), "+3".to_string());
        values.insert("managed".to_string(), "true".to_string());
        let toml = form::to_toml(&fields, &values);
        assert!(toml.contains("outputs = 3\n"), "{toml}");
        assert!(toml.contains("managed = true\n"), "{toml}");
        Profile::check(&toml).unwrap();
    }

    /// The Gyro field offers every button a profile can name, the back
    /// buttons among them, each read under "Assist holds".
    #[test]
    fn the_gyro_field_offers_the_back_buttons() {
        let fields = form::fields();
        let gyro = fields
            .iter()
            .find(|field| field.key == "assist-gyro")
            .unwrap();
        let Kind::Choice { choices } = &gyro.kind else {
            panic!("{gyro:?}");
        };
        let read: Vec<(&str, &str, &str, &str)> = choices
            .iter()
            .map(|choice| {
                (
                    choice.value.as_str(),
                    choice.label.as_str(),
                    choice.caption.as_str(),
                    choice.brief.as_str(),
                )
            })
            .collect();
        // After no button and the sixteen there were before them, last.
        assert_eq!(read.len(), 21, "{read:?}");
        assert_eq!(read[0].0, "none");
        assert_eq!(read[16].0, "touchpad");
        assert_eq!(
            read[17..],
            [
                ("paddle-1", "Assist · Paddle 1", "Assist holds", "Paddle 1"),
                ("paddle-2", "Assist · Paddle 2", "Assist holds", "Paddle 2"),
                ("paddle-3", "Assist · Paddle 3", "Assist holds", "Paddle 3"),
                ("paddle-4", "Assist · Paddle 4", "Assist holds", "Paddle 4"),
            ]
        );
    }

    #[test]
    fn a_profile_a_form_cannot_show_reads_as_nothing() {
        let fields = form::fields();
        for toml in [
            "not toml =",
            // The full form.
            include_str!("fixtures/full.toml"),
            // A preset with a key that is no field.
            "mode = \"priority\"\nprimary = \"a\"\nassist = \"b\"\n[source.x]\npad = \"c\"\n",
            // A field holding what no field holds.
            "mode = \"priority\"\nprimary = [\"a\"]\n",
            "primary = \"a\"\n",
        ] {
            assert_eq!(form::from_toml(&fields, toml), None, "{toml}");
        }
        // The file's own schema number is no field, and is passed over
        // where it is this version's.
        let preset = include_str!("fixtures/preset.toml");
        assert!(form::from_toml(&fields, preset).is_some());
        assert!(form::from_toml(&fields, &format!("schema = 1\n{preset}")).is_some());
    }

    /// A form shows nothing of a profile for another schema: what it
    /// writes carries no schema, so writing a newer profile's values
    /// back would quietly make it this version's.
    #[test]
    fn a_form_shows_nothing_of_a_profile_for_another_schema() {
        let fields = form::fields();
        let preset = include_str!("fixtures/preset.toml");
        for schema in ["2", "0", "\"1\""] {
            let toml = format!("schema = {schema}\n{preset}");
            assert_eq!(form::from_toml(&fields, &toml), None, "{toml}");
        }
    }

    #[test]
    fn the_description_travels_as_json_and_back() {
        let fields = form::fields();
        let json = serde_json::to_string(&fields).unwrap();
        assert!(json.contains("\"kind\":\"pad\""), "{json}");
        assert!(
            json.contains("\"kind\":\"number\",\"min\":1,\"max\":8"),
            "{json}"
        );
        let back: Vec<form::Field> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, fields);
    }
}
