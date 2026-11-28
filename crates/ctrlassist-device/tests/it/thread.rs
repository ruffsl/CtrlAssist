//! The input thread, end to end through the kernel: fake pads in, a
//! virtual pad out, a stand-in game asking for rumble.

use std::fs::File;
use std::os::fd::OwnedFd;
use std::sync::mpsc::Receiver;

use ctrlassist_core::{
    Effect, Graph, GraphSpec, Input, Layout, Mode, MuxConfig, NodeId, NodeSpec, PadState, SourceId,
    SourceSpec, gamepad,
};
use ctrlassist_device::{
    Command, Identity, InputThread, NewGraph, Pad, Place, Report, Request, Setup, SetupId,
    VirtualPad,
};
use ctrlassist_test::FakePad;
use rustix::event::{PollFd, PollFlags, Timespec, poll};

use crate::virtual_pad::{game_opens, rumble, writable};
use crate::{can_make_pads, opened, read_until};

const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const ABS_X: u16 = 0x00;
const BTN_SOUTH: u16 = 0x130;
const BTN_EAST: u16 = 0x131;

pub(crate) const PRIMARY: SourceId = SourceId(0);
const ASSIST: SourceId = SourceId(1);
pub(crate) const ID: SetupId = SetupId(7);

fn source() -> SourceSpec {
    SourceSpec {
        deadzone: 0.1,
        layout: Layout::gamepad(),
    }
}

/// Two pads into one virtual pad, the assist outranking the primary.
pub(crate) fn mux(mode: Mode) -> Graph {
    Graph::build(GraphSpec {
        sources: vec![source(), source()],
        nodes: vec![NodeSpec::Mux {
            inputs: vec![Input::Source(PRIMARY), Input::Source(ASSIST)],
            config: MuxConfig::new(mode, &Layout::gamepad()),
        }],
        sinks: vec![Input::Node(NodeId(0), 0)],
    })
    .unwrap()
}

pub(crate) fn read(fake: &FakePad) -> Pad {
    Pad::from_fd(opened(fake.event_node())).unwrap()
}

/// A virtual pad, and a reader on it standing in for the game.
pub(crate) fn sink() -> (VirtualPad, Pad) {
    let sink = VirtualPad::create(Identity::Generic).unwrap();
    let fd = opened(sink.event_node());
    (sink, Pad::from_fd(fd).unwrap())
}

struct Rig {
    primary: FakePad,
    assist: FakePad,
    game: Pad,
    thread: InputThread,
    reports: Receiver<Report>,
}

fn rig(mode: Mode) -> Rig {
    let primary = FakePad::new("Thread Test Primary", 0x1209, 0x5eed).unwrap();
    let assist = FakePad::new("Thread Test Assist", 0x1209, 0x5eed).unwrap();
    let (sink, game) = sink();
    let setup = Setup::new(
        mux(mode),
        vec![Some(read(&primary)), Some(read(&assist))],
        vec![sink],
    );
    let (thread, reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();
    Rig {
        primary,
        assist,
        game,
        thread,
        reports,
    }
}

pub(crate) fn last(frames: &[PadState]) -> PadState {
    *frames.last().unwrap()
}

#[test]
fn two_pads_reach_the_game_as_one_with_the_assist_outranking_the_primary() {
    if !can_make_pads("two_pads_reach_the_game_as_one_with_the_assist_outranking_the_primary") {
        return;
    }
    let mut rig = rig(Mode::Priority);

    rig.primary
        .frame(&[(EV_KEY, BTN_SOUTH, 1), (EV_ABS, ABS_X, 32767)])
        .unwrap();
    let held = PadState::NEUTRAL
        .with(gamepad::SOUTH, 1.0)
        .with(gamepad::LEFT_X, 1.0);
    let frames = read_until(&mut rig.game, |frames| frames.last() == Some(&held));
    assert_eq!(last(&frames), held);

    // The assist takes the stick and adds a button of its own.
    rig.assist
        .frame(&[(EV_KEY, BTN_EAST, 1), (EV_ABS, ABS_X, -32768)])
        .unwrap();
    let taken = held.with(gamepad::EAST, 1.0).with(gamepad::LEFT_X, -1.0);
    let frames = read_until(&mut rig.game, |frames| frames.last() == Some(&taken));
    assert_eq!(last(&frames), taken);

    // And gives it back by letting go.
    rig.assist
        .frame(&[(EV_KEY, BTN_EAST, 0), (EV_ABS, ABS_X, 0)])
        .unwrap();
    let frames = read_until(&mut rig.game, |frames| frames.last() == Some(&held));
    assert_eq!(last(&frames), held);
}

#[test]
fn a_press_and_release_in_separate_frames_both_reach_the_game() {
    if !can_make_pads("a_press_and_release_in_separate_frames_both_reach_the_game") {
        return;
    }
    let mut rig = rig(Mode::Priority);
    // Faster than anything reads them: both frames wait in the kernel.
    for _ in 0..5 {
        rig.primary.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
        rig.primary.frame(&[(EV_KEY, BTN_SOUTH, 0)]).unwrap();
    }
    let frames = read_until(&mut rig.game, |frames| frames.len() >= 10);
    let pressed = PadState::NEUTRAL.with(gamepad::SOUTH, 1.0);
    let expected: Vec<PadState> = (0..5).flat_map(|_| [pressed, PadState::NEUTRAL]).collect();
    assert_eq!(frames, expected);
}

#[test]
fn a_setup_starts_with_what_its_pads_already_hold() {
    if !can_make_pads("a_setup_starts_with_what_its_pads_already_hold") {
        return;
    }
    let mut primary = FakePad::new("Thread Test Held", 0x1209, 0x5eed).unwrap();
    primary.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
    let assist = FakePad::new("Thread Test Assist", 0x1209, 0x5eed).unwrap();
    let (sink, mut game) = sink();
    let setup = Setup::new(
        mux(Mode::Priority),
        vec![Some(read(&primary)), Some(read(&assist))],
        vec![sink],
    );
    let (thread, _reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();

    let held = PadState::NEUTRAL.with(gamepad::SOUTH, 1.0);
    let frames = read_until(&mut game, |frames| frames.last() == Some(&held));
    assert_eq!(last(&frames), held);
}

#[test]
fn a_switch_command_hands_a_toggle_to_the_other_pad() {
    if !can_make_pads("a_switch_command_hands_a_toggle_to_the_other_pad") {
        return;
    }
    let mut rig = rig(Mode::Toggle);
    // What a node that selects has selected is reported as it changes,
    // from the setup's start on.
    let selected = |rig: &Rig| match rig.reports.recv().unwrap() {
        Report::Selected { setup, node, index } => {
            assert_eq!((setup, node), (ID, NodeId(0)));
            index
        }
        other => unreachable!("expected a selection, got {other:?}"),
    };
    assert_eq!(selected(&rig), 0);
    rig.primary.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
    rig.assist.frame(&[(EV_KEY, BTN_EAST, 1)]).unwrap();
    let primary_only = PadState::NEUTRAL.with(gamepad::SOUTH, 1.0);
    let frames = read_until(&mut rig.game, |frames| frames.last() == Some(&primary_only));
    assert_eq!(
        last(&frames),
        primary_only,
        "a toggle starts on its first input"
    );

    rig.thread
        .send(Command::Switch {
            setup: ID,
            node: NodeId(0),
        })
        .unwrap();
    let assist_only = PadState::NEUTRAL.with(gamepad::EAST, 1.0);
    let frames = read_until(&mut rig.game, |frames| frames.last() == Some(&assist_only));
    assert_eq!(last(&frames), assist_only);
    assert_eq!(selected(&rig), 1);

    rig.thread
        .send(Command::Select {
            setup: ID,
            node: NodeId(0),
            index: 0,
        })
        .unwrap();
    let frames = read_until(&mut rig.game, |frames| frames.last() == Some(&primary_only));
    assert_eq!(last(&frames), primary_only);
    assert_eq!(selected(&rig), 0);

    // Selecting what is selected already is nothing to report; the next
    // report is the setup coming back.
    rig.thread
        .send(Command::Select {
            setup: ID,
            node: NodeId(0),
            index: 0,
        })
        .unwrap();
    rig.thread.send(Command::Stop(ID)).unwrap();
    assert!(matches!(
        rig.reports.recv().unwrap(),
        Report::Stopped(ID, _)
    ));
}

/// Each of the rig's two sources keeping the pad it has.
fn same() -> Vec<Place> {
    vec![Place::Keep(PRIMARY), Place::Keep(ASSIST)]
}

/// A toggle over the two pads in the order given.
fn toggle(inputs: [SourceId; 2]) -> Graph {
    Graph::build(GraphSpec {
        sources: vec![source(), source()],
        nodes: vec![NodeSpec::Mux {
            inputs: inputs.into_iter().map(Input::Source).collect(),
            config: MuxConfig::new(Mode::Toggle, &Layout::gamepad()),
        }],
        sinks: vec![Input::Node(NodeId(0), 0)],
    })
    .unwrap()
}

#[test]
fn a_running_setup_takes_a_new_graph_and_the_game_keeps_its_pad() {
    if !can_make_pads("a_running_setup_takes_a_new_graph_and_the_game_keeps_its_pad") {
        return;
    }
    let mut rig = rig(Mode::Priority);
    rig.primary.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
    rig.assist.frame(&[(EV_KEY, BTN_EAST, 1)]).unwrap();
    let both = PadState::NEUTRAL
        .with(gamepad::SOUTH, 1.0)
        .with(gamepad::EAST, 1.0);
    let frames = read_until(&mut rig.game, |frames| frames.last() == Some(&both));
    assert_eq!(last(&frames), both);

    // A toggle in the priority mux's place: the game's pad stays, and
    // changes only where the two graphs disagree about what is held.
    let graph = Box::new(NewGraph::new(toggle([PRIMARY, ASSIST]), vec![None], same()));
    rig.thread
        .send(Command::NewGraph { setup: ID, graph })
        .unwrap();
    let primary_only = PadState::NEUTRAL.with(gamepad::SOUTH, 1.0);
    let frames = read_until(&mut rig.game, |frames| frames.last() == Some(&primary_only));
    assert_eq!(last(&frames), primary_only);
    // The graph first, then what its nodes select.
    assert!(matches!(
        rig.reports.recv().unwrap(),
        Report::GraphTaken { setup: ID, .. }
    ));
    assert!(matches!(
        rig.reports.recv().unwrap(),
        Report::Selected {
            setup: ID,
            node: NodeId(0),
            index: 0
        }
    ));

    // The new graph is what the pads now play through.
    rig.thread
        .send(Command::Switch {
            setup: ID,
            node: NodeId(0),
        })
        .unwrap();
    let assist_only = PadState::NEUTRAL.with(gamepad::EAST, 1.0);
    let frames = read_until(&mut rig.game, |frames| frames.last() == Some(&assist_only));
    assert_eq!(last(&frames), assist_only);
    assert!(matches!(
        rig.reports.recv().unwrap(),
        Report::Selected { index: 1, .. }
    ));

    // A node that carries on goes on selecting what it had: the assist,
    // which is the new node's first input, where a toggle left to itself
    // starts on its first and would here have been told nothing.
    let carry = vec![Some((NodeId(0), vec![Some(1), Some(0)]))];
    let graph = Box::new(NewGraph::new(toggle([ASSIST, PRIMARY]), carry, same()));
    rig.thread
        .send(Command::NewGraph { setup: ID, graph })
        .unwrap();
    assert!(matches!(
        rig.reports.recv().unwrap(),
        Report::GraphTaken { setup: ID, .. }
    ));
    assert!(matches!(
        rig.reports.recv().unwrap(),
        Report::Selected { index: 0, .. }
    ));
    // And back in the first order, the assist is its second.
    let carry = vec![Some((NodeId(0), vec![Some(1), Some(0)]))];
    let graph = Box::new(NewGraph::new(toggle([PRIMARY, ASSIST]), carry, same()));
    rig.thread
        .send(Command::NewGraph { setup: ID, graph })
        .unwrap();
    assert!(matches!(
        rig.reports.recv().unwrap(),
        Report::GraphTaken { setup: ID, .. }
    ));
    assert!(matches!(
        rig.reports.recv().unwrap(),
        Report::Selected { index: 1, .. }
    ));
    rig.primary.frame(&[(EV_ABS, ABS_X, 32767)]).unwrap();
    rig.assist.frame(&[(EV_KEY, BTN_EAST, 0)]).unwrap();
    let frames = read_until(&mut rig.game, |frames| {
        frames.last() == Some(&PadState::NEUTRAL)
    });
    assert_eq!(
        last(&frames),
        PadState::NEUTRAL,
        "the assist still has the toggle, so the primary's stick is not heard"
    );
}

#[test]
fn a_running_setup_takes_other_pads_and_the_game_keeps_its_pad() {
    if !can_make_pads("a_running_setup_takes_other_pads_and_the_game_keeps_its_pad") {
        return;
    }
    let mut rig = rig(Mode::Priority);
    let mut third = FakePad::new("Thread Test Third", 0x1209, 0x5eed).unwrap();
    rig.primary.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
    let south = PadState::NEUTRAL.with(gamepad::SOUTH, 1.0);
    let frames = read_until(&mut rig.game, |frames| frames.last() == Some(&south));
    assert_eq!(last(&frames), south);

    // The assist becomes the primary, and a third pad the assist: the
    // old primary is handed back, and what it held is let go of on the
    // game's pad, which is the one it had.
    let places = vec![
        Place::Keep(ASSIST),
        Place::Take(Some(Box::new(read(&third)))),
    ];
    let graph = Box::new(NewGraph::new(mux(Mode::Priority), vec![None], places));
    rig.thread
        .send(Command::NewGraph { setup: ID, graph })
        .unwrap();
    let frames = read_until(&mut rig.game, |frames| {
        frames.last() == Some(&PadState::NEUTRAL)
    });
    assert_eq!(last(&frames), PadState::NEUTRAL);
    assert!(matches!(
        rig.reports.recv().unwrap(),
        Report::GraphTaken { setup: ID, .. }
    ));

    // The pad given up is no longer heard; the two it has are, each as
    // the source it now is.
    rig.primary.frame(&[(EV_KEY, BTN_SOUTH, 0)]).unwrap();
    rig.primary.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
    third.frame(&[(EV_KEY, BTN_EAST, 1)]).unwrap();
    let east = PadState::NEUTRAL.with(gamepad::EAST, 1.0);
    let frames = read_until(&mut rig.game, |frames| frames.last() == Some(&east));
    assert_eq!(last(&frames), east);
    rig.assist.frame(&[(EV_ABS, ABS_X, 32767)]).unwrap();
    let frames = read_until(&mut rig.game, |frames| {
        frames
            .last()
            .is_some_and(|state| state.get(gamepad::LEFT_X) > 0.9)
    });
    assert!(last(&frames).get(gamepad::EAST) > 0.5);

    // The pad that arrived going is reported as the source it is.
    drop(third);
    assert!(matches!(
        rig.reports.recv().unwrap(),
        Report::PadGone {
            setup: ID,
            source: ASSIST,
            ..
        }
    ));
    // A source can be left without a pad, to be given one later.
    let places = vec![Place::Take(None), Place::Keep(PRIMARY)];
    let graph = Box::new(NewGraph::new(mux(Mode::Priority), vec![None], places));
    rig.thread
        .send(Command::NewGraph { setup: ID, graph })
        .unwrap();
    assert!(matches!(
        rig.reports.recv().unwrap(),
        Report::GraphTaken { setup: ID, .. }
    ));
    rig.assist.frame(&[(EV_ABS, ABS_X, 0)]).unwrap();
    let frames = read_until(&mut rig.game, |frames| {
        frames.last() == Some(&PadState::NEUTRAL)
    });
    assert_eq!(last(&frames), PadState::NEUTRAL);
}

#[test]
fn a_graph_of_other_pads_or_for_no_setup_is_handed_back() {
    if !can_make_pads("a_graph_of_other_pads_or_for_no_setup_is_handed_back") {
        return;
    }
    let rig = rig(Mode::Priority);
    let one_pad = Graph::build(GraphSpec {
        sources: vec![source()],
        nodes: vec![],
        sinks: vec![Input::Source(PRIMARY)],
    })
    .unwrap();
    // Its parts don't fit each other: one source, two places.
    let graph = Box::new(NewGraph::new(one_pad, vec![], same()));
    rig.thread
        .send(Command::NewGraph { setup: ID, graph })
        .unwrap();
    assert!(matches!(
        rig.reports.recv().unwrap(),
        Report::GraphRefused { setup: ID, .. }
    ));
    // It keeps a pad the setup has no source for.
    let places = vec![Place::Keep(PRIMARY), Place::Keep(SourceId(5))];
    let graph = Box::new(NewGraph::new(mux(Mode::Average), vec![None], places));
    rig.thread
        .send(Command::NewGraph { setup: ID, graph })
        .unwrap();
    assert!(matches!(
        rig.reports.recv().unwrap(),
        Report::GraphRefused { setup: ID, .. }
    ));
    let nobody = SetupId(99);
    let graph = Box::new(NewGraph::new(mux(Mode::Average), vec![None], same()));
    rig.thread
        .send(Command::NewGraph {
            setup: nobody,
            graph,
        })
        .unwrap();
    assert!(matches!(
        rig.reports.recv().unwrap(),
        Report::GraphRefused { setup, .. } if setup == nobody
    ));
}

#[test]
fn a_pad_that_goes_is_reported_and_the_setup_carries_on_without_it() {
    if !can_make_pads("a_pad_that_goes_is_reported_and_the_setup_carries_on_without_it") {
        return;
    }
    let Rig {
        mut primary,
        mut assist,
        mut game,
        thread: _thread,
        reports,
    } = rig(Mode::Priority);
    primary.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
    read_until(&mut game, |frames| !frames.is_empty());

    drop(primary);
    match reports.recv().unwrap() {
        Report::PadGone { setup, source, .. } => assert_eq!((setup, source), (ID, PRIMARY)),
        other => unreachable!("expected the pad to be reported gone, got {other:?}"),
    }
    // What the missing pad held is let go, not left held forever.
    let frames = read_until(&mut game, |frames| {
        frames.last() == Some(&PadState::NEUTRAL)
    });
    assert_eq!(last(&frames), PadState::NEUTRAL);

    assist.frame(&[(EV_KEY, BTN_EAST, 1)]).unwrap();
    let frames = read_until(&mut game, |frames| !frames.is_empty());
    assert_eq!(last(&frames), PadState::NEUTRAL.with(gamepad::EAST, 1.0));
}

#[test]
fn stopping_a_setup_hands_it_back_and_takes_its_virtual_pad_away() {
    if !can_make_pads("stopping_a_setup_hands_it_back_and_takes_its_virtual_pad_away") {
        return;
    }
    let rig = rig(Mode::Priority);
    rig.thread.send(Command::Stop(ID)).unwrap();
    let setup = match rig.reports.recv().unwrap() {
        Report::Stopped(id, setup) => {
            assert_eq!(id, ID);
            setup
        }
        other => unreachable!("expected the setup back, got {other:?}"),
    };
    assert_eq!(setup.pads().len(), 2);
    let node = setup.sinks()[0].event_node().to_path_buf();
    let _ = std::process::Command::new("udevadm").arg("settle").status();
    let phys = ctrlassist_test::phys_of(&node).expect("the virtual pad is connected");
    // The virtual pad lives exactly as long as the setup does. Its node's
    // path may be another pad's by now, so it is known by its own mark.
    drop(setup);
    let _ = std::process::Command::new("udevadm").arg("settle").status();
    assert!(!ctrlassist_test::phys_connected(&phys));

    // A stop for a setup that isn't running is nothing.
    rig.thread.send(Command::Stop(ID)).unwrap();
}

#[test]
fn starting_a_setup_twice_under_one_name_fails_the_second() {
    if !can_make_pads("starting_a_setup_twice_under_one_name_fails_the_second") {
        return;
    }
    let rig = rig(Mode::Priority);
    let second = Setup::new(mux(Mode::Priority), vec![None, None], Vec::new());
    rig.thread
        .send(Command::Start(ID, Box::new(second)))
        .unwrap();
    match rig.reports.recv().unwrap() {
        Report::Failed(id, _) => assert_eq!(id, ID),
        other => unreachable!("expected the second start to fail, got {other:?}"),
    }
}

#[test]
fn dropping_the_thread_stops_it_and_hands_back_what_was_running() {
    if !can_make_pads("dropping_the_thread_stops_it_and_hands_back_what_was_running") {
        return;
    }
    // The fake pads are kept: a pad going would be reported first.
    let Rig {
        thread,
        reports,
        primary: _primary,
        assist: _assist,
        game: _game,
    } = rig(Mode::Priority);
    drop(thread);
    assert!(matches!(reports.recv().unwrap(), Report::Stopped(ID, _)));
    assert!(reports.recv().is_err(), "the thread has ended");
}

#[test]
fn a_game_s_rumble_reaches_the_pad_and_ends_when_the_game_said() {
    if !can_make_pads("a_game_s_rumble_reaches_the_pad_and_ends_when_the_game_said") {
        return;
    }
    // A virtual pad stands in for a real pad with motors, read and
    // rumbled through its event node as a real one is.
    let mut motors = VirtualPad::create(Identity::Generic).unwrap();
    let _ = std::process::Command::new("udevadm").arg("settle").status();
    // Opening the pad uploads the one effect it keeps, which the stand-in
    // answers as a pad's driver would.
    let node = motors.event_node().to_path_buf();
    let opening = std::thread::spawn(move || writable(&node));
    let mut requests = Vec::new();
    while !opening.is_finished() {
        let mut fds = [PollFd::new(&motors, PollFlags::IN)];
        let wait = Timespec {
            tv_sec: 0,
            tv_nsec: 10_000_000,
        };
        let _ = poll(&mut fds, Some(&wait));
        motors
            .serve(|request| {
                requests.push(request);
                Ok(())
            })
            .unwrap();
    }
    let pad = opening.join().unwrap();

    let (sink, _reader) = sink();
    let game_node = sink.event_node().to_path_buf();
    let graph = Graph::build(GraphSpec {
        sources: vec![source()],
        nodes: Vec::new(),
        sinks: vec![Input::Source(PRIMARY)],
    })
    .unwrap();
    let (thread, reports) = InputThread::spawn().unwrap();
    thread
        .send(Command::Start(
            ID,
            Box::new(Setup::new(graph, vec![Some(pad)], vec![sink])),
        ))
        .unwrap();

    let started = std::time::Instant::now();
    let game = std::thread::spawn(move || {
        let mut pad = game_opens(&game_node);
        let mut effect = pad
            .upload_ff_effect(rumble(0x6000, 0x3000, 300, 0))
            .unwrap();
        effect.play(1).unwrap();
        // Hold the effect: the game doesn't stop it, its length does.
        std::thread::park();
        drop(effect);
    });

    // When the pad was seen told to play, and how long after that it was
    // seen told to stop.
    let (mut played, mut ran) = (None, None);
    let mut ended = None;
    for _ in 0..400 {
        let mut fds = [PollFd::new(&motors, PollFlags::IN)];
        let wait = Timespec {
            tv_sec: 0,
            tv_nsec: 10_000_000,
        };
        let _ = poll(&mut fds, Some(&wait));
        motors
            .serve(|request| {
                requests.push(request);
                Ok(())
            })
            .unwrap();
        if played.is_none()
            && requests
                .iter()
                .any(|request| matches!(request, Request::Play { count: 1.., .. }))
        {
            played = Some(std::time::Instant::now());
        }
        // The pad's effect is stopped, and kept for its next rumble.
        if requests
            .iter()
            .any(|request| matches!(request, Request::Play { count: 0, .. }))
        {
            ended = Some(started.elapsed());
            ran = played.map(|played| played.elapsed());
            break;
        }
    }
    game.thread().unpark();
    game.join().unwrap();

    // Stopped, the setup is handed back, and dropping it drops the pad,
    // whose own thread then erases the effect it kept. The stand-in
    // answers that as a pad's driver does: unanswered, the kernel holds
    // that thread 30 seconds, and again as the pad's node is closed, but
    // not whoever dropped the pad, which here is the stand-in's server.
    thread.send(Command::Stop(ID)).unwrap();
    let setup = match reports.recv().unwrap() {
        Report::Stopped(_, setup) => setup,
        other => unreachable!("expected the setup back, got {other:?}"),
    };
    drop(setup);
    let mut erased = false;
    for _ in 0..400 {
        let mut fds = [PollFd::new(&motors, PollFlags::IN)];
        let wait = Timespec {
            tv_sec: 0,
            tv_nsec: 10_000_000,
        };
        let _ = poll(&mut fds, Some(&wait));
        motors
            .serve(|request| {
                erased |= request == Request::Erase { id: 0 };
                Ok(())
            })
            .unwrap();
        if erased {
            break;
        }
    }
    assert!(erased, "the pad's effect is erased as the pad is dropped");

    let level = Effect {
        strong: 0x6000,
        weak: 0x3000,
        length_ms: 0,
        delay_ms: 0,
    };
    assert_eq!(
        requests,
        [
            Request::Upload {
                id: 0,
                effect: Effect {
                    strong: 0,
                    weak: 0,
                    ..level
                }
            },
            Request::Upload {
                id: 0,
                effect: level
            },
            Request::Play { id: 0, count: 1 },
            Request::Play { id: 0, count: 0 },
        ]
    );
    let ended = ended.unwrap().as_millis();
    // No sooner than the game said: that clock started before the game
    // played it, and times are kept in whole milliseconds, so allow a
    // little under 300.
    assert!(ended >= 290, "a 300 ms rumble ended after {ended} ms");
    // And not long after, by the clock started when the pad was seen
    // playing: the game opening its pad and uploading the effect, which a
    // busy machine is slow at, is no part of it.
    let ran = ran.unwrap().as_millis();
    assert!(ran < 600, "a 300 ms rumble ran for {ran} ms");
}

/// How many times `thread` has been scheduled out, by its own choice or
/// the kernel's: that thread, and not another test's input thread.
pub(crate) fn input_thread_switches(thread: &InputThread) -> u64 {
    let task = thread.task().unwrap();
    std::fs::read_to_string(format!("/proc/self/task/{task}/status"))
        .unwrap()
        .lines()
        .filter(|line| line.contains("ctxt_switches"))
        .map(|line| {
            line.split_whitespace()
                .nth(1)
                .unwrap()
                .parse::<u64>()
                .unwrap()
        })
        .sum()
}

/// Waits until `thread` sleeps: the kernel's own word for it, the state
/// after the name in the task's `stat`. The input thread waits nowhere
/// but for something to do.
fn wait_until_asleep(thread: &InputThread) {
    let task = thread.task().unwrap();
    loop {
        let stat = std::fs::read_to_string(format!("/proc/self/task/{task}/stat")).unwrap();
        // The name is in parentheses and may hold anything; the state is
        // the field after the last of them.
        let state = stat.rsplit_once(')').unwrap().1.trim_start().chars().next();
        if state == Some('S') {
            return;
        }
        std::thread::yield_now();
    }
}

#[test]
fn an_idle_input_thread_never_wakes() {
    if !can_make_pads("an_idle_input_thread_never_wakes") {
        return;
    }
    let mut rig = rig(Mode::Priority);
    // Something through it first, so it is known to be running.
    rig.primary.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
    read_until(&mut rig.game, |frames| !frames.is_empty());

    // The game has its frame before the thread is back waiting, and going
    // to wait is itself counted.
    wait_until_asleep(&rig.thread);
    let before = input_thread_switches(&rig.thread);
    std::thread::sleep(std::time::Duration::from_secs(1));
    assert_eq!(
        input_thread_switches(&rig.thread),
        before,
        "the input thread woke with nothing to do"
    );
}

/// Tapped, the thread says what a game asks of a virtual pad and what
/// the pads are told; untapped, it says nothing of either.
#[test]
fn a_tapped_thread_says_what_passes_through_a_setup_s_feedback() {
    if !can_make_pads("a_tapped_thread_says_what_passes_through_a_setup_s_feedback") {
        return;
    }
    use ctrlassist_device::Tap;
    let primary = FakePad::new("Thread Tap Primary", 0x1209, 0x5eed).unwrap();
    let (sink, _game) = sink();
    let node = sink.event_node().to_path_buf();
    let graph = Graph::build(GraphSpec {
        sources: vec![source()],
        nodes: vec![],
        sinks: vec![Input::Source(PRIMARY)],
    })
    .unwrap();
    let setup = Setup::new(graph, vec![Some(read(&primary))], vec![sink]);
    let (thread, reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();
    let mut pad = game_opens(&node);
    let mut play = |strong| {
        let mut effect = pad.upload_ff_effect(rumble(strong, 0, 0, 0)).unwrap();
        effect.play(1).unwrap();
        effect
    };
    // Untapped: a rumble comes and goes unremarked.
    drop(play(0x1000));
    thread.tap(true);
    let _held = play(0x8000);
    // The pad has no motors, and is told its level all the same. What
    // was still on its way from before the tap may come first.
    let told = Tap::Told {
        source: PRIMARY,
        level: ctrlassist_core::Level {
            strong: 0x8000,
            weak: 0,
        },
    };
    let mut seen = Vec::new();
    while seen.last() != Some(&told) {
        match reports.recv().unwrap() {
            Report::Tapped { setup: ID, tap } => seen.push(tap),
            other => unreachable!("expected a tap, got {other:?}"),
        }
    }
    let uploaded = seen.iter().position(|tap| {
        matches!(
            tap,
            Tap::Asked {
                request: Request::Upload { effect, .. },
                ..
            } if effect.strong == 0x8000
        )
    });
    let played = seen.iter().rposition(|tap| {
        matches!(
            tap,
            Tap::Asked {
                request: Request::Play { count: 1, .. },
                ..
            }
        )
    });
    assert!(
        uploaded.is_some() && uploaded < played,
        "the upload, then the play, then the level: {seen:?}"
    );
}

#[test]
fn a_buzz_outranks_a_game_s_rumble_and_then_gives_the_pad_back_to_it() {
    if !can_make_pads("a_buzz_outranks_a_game_s_rumble_and_then_gives_the_pad_back_to_it") {
        return;
    }
    let motors = ctrlassist_test::RumblePad::new("Thread Buzz Pad", 0x1209, 0x5eef).unwrap();
    let pad = writable(motors.event_node());
    let (sink, _reader) = sink();
    let game_node = sink.event_node().to_path_buf();
    let graph = Graph::build(GraphSpec {
        sources: vec![source()],
        nodes: Vec::new(),
        sinks: vec![Input::Source(PRIMARY)],
    })
    .unwrap();
    let (thread, _reports) = InputThread::spawn().unwrap();
    thread
        .send(Command::Start(
            ID,
            Box::new(Setup::new(graph, vec![Some(pad)], vec![sink])),
        ))
        .unwrap();
    // What the motors were set to, leaving out the stops between levels.
    let levels = || -> Vec<(u16, u16)> {
        motors
            .motors()
            .into_iter()
            .filter(|level| *level != (0, 0))
            .collect()
    };
    let wait_for = |count: usize| {
        for _ in 0..500 {
            if levels().len() >= count {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        unreachable!("the motors were set {:?}", motors.motors());
    };

    // A game rumbles for longer than the test lasts.
    let game = std::thread::spawn(move || {
        let mut pad = game_opens(&game_node);
        let mut effect = pad
            .upload_ff_effect(rumble(0x6000, 0x3000, 30_000, 0))
            .unwrap();
        effect.play(1).unwrap();
        std::thread::park();
        drop(effect);
    });
    wait_for(1);

    let buzz = ctrlassist_core::Level {
        strong: 0xa000,
        weak: 0xa000,
    };
    let asked = std::time::Instant::now();
    thread
        .send(Command::Buzz {
            setup: ID,
            source: PRIMARY,
            level: buzz,
            millis: 200,
        })
        .unwrap();
    wait_for(3);
    let lasted = asked.elapsed();
    assert_eq!(
        levels(),
        [(0x6000, 0x3000), (0xa000, 0xa000), (0x6000, 0x3000)]
    );
    assert!(
        lasted >= std::time::Duration::from_millis(200),
        "a 200 ms buzz ended after {lasted:?}"
    );

    // A buzz for a pad the setup doesn't have, or a setup that isn't
    // running, is nothing.
    for (setup, source) in [(ID, SourceId(5)), (SetupId(9), PRIMARY)] {
        thread
            .send(Command::Buzz {
                setup,
                source,
                level: buzz,
                millis: 50,
            })
            .unwrap();
    }
    game.thread().unpark();
    game.join().unwrap();
    assert_eq!(levels().len(), 3);
}

/// A pad another program makes through uinput, as Steam Input or a
/// game-streaming host does, has its effect updated by asking that
/// program, and the kernel holds whoever asks until it answers, up to 30
/// seconds. One that has stopped answering holds only its own pad's
/// rumble: every other setup's input still flows, commands are still
/// taken, and the level goes to the pad once its maker answers again.
#[test]
fn a_pad_whose_maker_stops_answering_holds_nothing_but_its_own_rumble() {
    if !can_make_pads("a_pad_whose_maker_stops_answering_holds_nothing_but_its_own_rumble") {
        return;
    }
    const OTHER: SetupId = SetupId(8);
    let motors = ctrlassist_test::RumblePad::new("Thread Hung Pad", 0x1209, 0x5ef0).unwrap();
    // Its one effect is uploaded as it is opened, which it answers.
    let pad = writable(motors.event_node());
    motors.stop_answering();
    let one = || {
        Graph::build(GraphSpec {
            sources: vec![source()],
            nodes: Vec::new(),
            sinks: vec![Input::Source(PRIMARY)],
        })
        .unwrap()
    };
    let (sink, _reader) = sink();
    let game_node = sink.event_node().to_path_buf();
    let mut other = FakePad::new("Thread Other Primary", 0x1209, 0x5eed).unwrap();
    let (other_sink, mut other_game) = self::sink();
    let (thread, reports) = InputThread::spawn().unwrap();
    for (id, pad, sink) in [(ID, pad, sink), (OTHER, read(&other), other_sink)] {
        let setup = Setup::new(one(), vec![Some(pad)], vec![sink]);
        thread.send(Command::Start(id, Box::new(setup))).unwrap();
    }

    // A game rumbles the hung pad's setup, and the pad's effect is asked
    // to take the level.
    let mut game = game_opens(&game_node);
    let mut effect = game.upload_ff_effect(rumble(0x7000, 0x1000, 0, 0)).unwrap();
    effect.play(1).unwrap();
    for _ in 0..400 {
        if motors.is_asked() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(motors.is_asked(), "the pad was never asked for the level");

    // While that waits, the other setup's input reaches its game.
    other.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
    let frames = read_until(&mut other_game, |frames| {
        frames
            .last()
            .is_some_and(|state| state.get(gamepad::SOUTH) > 0.0)
    });
    assert!(
        frames
            .last()
            .is_some_and(|state| state.get(gamepad::SOUTH) > 0.0),
        "the other setup's press never arrived: {frames:?}"
    );
    // And a command is taken.
    thread.send(Command::Stop(OTHER)).unwrap();
    match reports.recv().unwrap() {
        Report::Stopped(OTHER, _) => {}
        report => unreachable!("expected the other setup back, got {report:?}"),
    }

    // Answered again, the level reaches the motors.
    motors.answer_again();
    for _ in 0..400 {
        if motors.motors().contains(&(0x7000, 0x1000)) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(motors.motors(), [(0x7000, 0x1000)]);
    drop(effect);
}

/// A virtual DualSense reports all the time, as a real one does, with
/// nothing changing behind it: a program that opens a pad of that kind
/// and hears nothing takes it for one it can't rumble, or for gone.
#[test]
fn a_virtual_dualsense_in_a_setup_reports_all_the_time() {
    const TEST: &str = "a_virtual_dualsense_in_a_setup_reports_all_the_time";
    if !ctrlassist_test::can_make_hid_pads(TEST) {
        return;
    }
    let primary = FakePad::new("Thread Beat Primary", 0x1209, 0x5eed).unwrap();
    let data = ctrlassist_test::synthetic_dualsense(true);
    let sink = VirtualPad::dualsense(&data, ctrlassist_device::own_address(0x7e_5b01)).unwrap();
    let _ = std::process::Command::new("udevadm").arg("settle").status();
    let raw = ctrlassist_test::raw_node_of(sink.event_node()).unwrap();
    if !ctrlassist_test::can_open(&raw, false) {
        use std::io::Write as _;
        let _ = writeln!(
            std::io::stderr(),
            "SKIPPED {TEST}: a DualSense's raw node isn't this user's to open here"
        );
        return;
    }
    let graph = Graph::build(GraphSpec {
        sources: vec![source()],
        nodes: vec![],
        sinks: vec![Input::Source(PRIMARY)],
    })
    .unwrap();
    let setup = Setup::new(graph, vec![Some(read(&primary))], vec![sink]);
    let (thread, _reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();

    // A second of what the pad says, with no pad behind it touched.
    use std::io::Read as _;
    let mut reader = File::open(&raw).unwrap();
    let started = std::time::Instant::now();
    let (mut reports, mut last) = (0u32, [0u8; 64]);
    let mut first = None;
    let mut sequences = std::collections::BTreeSet::new();
    while started.elapsed() < std::time::Duration::from_secs(1) {
        let mut report = [0u8; 64];
        assert_eq!(reader.read(&mut report).unwrap(), 64);
        // The same state each time, told apart by its count and the time
        // its sensors were read.
        assert_eq!(report[..7], [1, 128, 128, 128, 128, 0, 0]);
        sequences.insert(report[7]);
        assert_ne!(report[28..32], last[28..32], "the sensor clock moves");
        first.get_or_insert(u32::from_le_bytes([
            report[28], report[29], report[30], report[31],
        ]));
        last = report;
        reports += 1;
    }
    // It went on reporting, each report counted once. How many a second
    // holds is the machine's to say: a busy one is late with some.
    assert!(reports >= 20, "{reports} reports in a second");
    assert_eq!(sequences.len(), reports as usize, "each has its own count");
    // And no faster than a real pad's beat of 10 ms, by the pad's own
    // sensor clock, which counts thirds of a microsecond.
    let clock =
        |report: &[u8; 64]| u32::from_le_bytes([report[28], report[29], report[30], report[31]]);
    let spacing = clock(&last).wrapping_sub(first.unwrap()) / (reports - 1) / 3;
    assert!(spacing >= 9_000, "a report every {spacing} µs");
}

/// A game's output reports each change only what they mark, and the pad
/// behind a virtual DualSense is sent everything the games have set: a
/// report that sets the lightbar, passed on, and then one that sets the
/// player LEDs reach it as one that sets both.
#[test]
fn a_pad_behind_a_virtual_dualsense_is_sent_everything_the_games_set() {
    const TEST: &str = "a_pad_behind_a_virtual_dualsense_is_sent_everything_the_games_set";
    if !ctrlassist_test::can_make_hid_pads(TEST) {
        return;
    }
    let skip = |why: &str| {
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), "SKIPPED {TEST}: {why}");
    };
    let real = ctrlassist_test::FakeDualSense::new(0x32).unwrap();
    let data = ctrlassist_test::synthetic_dualsense(true);
    let sink = VirtualPad::dualsense(&data, ctrlassist_device::own_address(0x7e_5b02)).unwrap();
    let _ = std::process::Command::new("udevadm").arg("settle").status();
    let real_raw = ctrlassist_test::raw_node_of(real.event_node()).unwrap();
    let sink_raw = ctrlassist_test::raw_node_of(sink.event_node()).unwrap();
    if !ctrlassist_test::can_open(&real_raw, true) || !ctrlassist_test::can_open(&sink_raw, true) {
        return skip("a DualSense's raw node isn't this user's to open here");
    }
    let mut pad = writable(real.event_node());
    let raw: OwnedFd = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&real_raw)
        .unwrap()
        .into();
    pad.set_raw(raw).unwrap();
    let graph = Graph::build(GraphSpec {
        sources: vec![source()],
        nodes: vec![],
        sinks: vec![Input::Source(PRIMARY)],
    })
    .unwrap();
    let setup = Setup::new(graph, vec![Some(pad)], vec![sink]);
    let (thread, _reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();

    use std::io::Write as _;
    let mut game = std::fs::OpenOptions::new()
        .write(true)
        .open(&sink_raw)
        .unwrap();
    let mut lights = [0u8; 48];
    lights[0] = 0x02;
    (lights[2], lights[45], lights[46], lights[47]) = (0x04, 1, 2, 3);
    let mut players = [0u8; 48];
    players[0] = 0x02;
    (players[2], players[44]) = (0x10, 0x05);
    game.write_all(&lights).unwrap();
    // The lightbar passed on alone first, so the input thread reads the
    // two apart, and the second report is all it has new.
    let lit = |sent: &Vec<u8>| sent[2] & 0x04 != 0 && sent[45..48] == [1, 2, 3];
    for _ in 0..400 {
        if real.outputs().iter().any(lit) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(real.outputs().iter().any(lit), "{:?}", real.outputs());
    game.write_all(&players).unwrap();
    let both =
        |sent: &Vec<u8>| sent[2] & 0x14 == 0x14 && sent[44] == 0x05 && sent[45..48] == [1, 2, 3];
    for _ in 0..400 {
        if real.outputs().iter().any(both) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    unreachable!("no report set both: {:?}", real.outputs());
}

/// A game asking for rumble many times over while the control side reads
/// nothing fills the input thread's queue with taps, and the reports that
/// hand something back still find room: each pad's as it goes, and the
/// setup's as it stops, since the room kept counts the setup's pads.
#[test]
fn a_flood_of_taps_leaves_room_for_the_pads_and_the_setup_handed_back() {
    if !can_make_pads("a_flood_of_taps_leaves_room_for_the_pads_and_the_setup_handed_back") {
        return;
    }
    use std::sync::{Arc, Condvar, Mutex};
    let mut primary = FakePad::new("Thread Flood Primary", 0x1209, 0x5eed).unwrap();
    let mut assist = FakePad::new("Thread Flood Assist", 0x1209, 0x5eed).unwrap();
    let (sink, mut reader) = sink();
    let node = sink.event_node().to_path_buf();
    let setup = Setup::new(
        mux(Mode::Priority),
        vec![Some(read(&primary)), Some(read(&assist))],
        vec![sink],
    );
    // The control side reads nothing until the gate opens.
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (passed, reports) = std::sync::mpsc::channel();
    let thread = {
        let gate = Arc::clone(&gate);
        InputThread::spawn_with(move |report| {
            let (open, opened) = &*gate;
            let mut open = open.lock().unwrap();
            while !*open {
                open = opened.wait(open).unwrap();
            }
            let _ = passed.send(report);
        })
        .unwrap()
    };
    thread.tap(true);
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();
    // Both pads hold the same button, which the game sees let go only
    // once both have gone and the thread has handled each going.
    primary.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
    assist.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
    let held = PadState::NEUTRAL.with(gamepad::SOUTH, 1.0);
    read_until(&mut reader, |frames| frames.last() == Some(&held));

    let mut game = game_opens(&node);
    for strong in 0..1000u16 {
        let mut effect = game.upload_ff_effect(rumble(strong, 0, 1000, 0)).unwrap();
        effect.play(1).unwrap();
    }
    drop(primary);
    drop(assist);
    let frames = read_until(&mut reader, |frames| {
        frames.last() == Some(&PadState::NEUTRAL)
    });
    assert_eq!(last(&frames), PadState::NEUTRAL);
    thread.send(Command::Stop(ID)).unwrap();

    {
        let (open, opened) = &*gate;
        *open.lock().unwrap() = true;
        opened.notify_all();
    }
    let (mut taps, mut gone) = (0, Vec::new());
    loop {
        match reports
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("every report that hands something back arrives")
        {
            Report::Tapped { .. } => taps += 1,
            Report::PadGone { source, .. } => gone.push(source),
            Report::Stopped(ID, _) => break,
            other => unreachable!("{other:?}"),
        }
    }
    gone.sort_by_key(|source| source.0);
    assert_eq!(gone, [PRIMARY, ASSIST]);
    // The queue holds 64 commands' worth beyond what may be handed back.
    assert!(taps > 64, "the taps filled what they may: {taps}");
}
