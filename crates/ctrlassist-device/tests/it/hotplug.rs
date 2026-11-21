//! Pads connecting, going and returning.

use ctrlassist_core::{Mode, PadState, gamepad};
use ctrlassist_device::{
    Change, Command, Found, Identifiers, InputThread, Monitor, PadId, Report, Setup, connected,
};
use ctrlassist_test::FakePad;
use rustix::event::{PollFd, PollFlags, poll};

use crate::thread::{ID, PRIMARY, last, mux, read, sink};
use crate::{can_make_pads, read_until};

const EV_KEY: u16 = 1;
const BTN_SOUTH: u16 = 0x130;
const BTN_EAST: u16 = 0x131;

/// Reads the monitor until `wanted` picks a change out.
fn wait_for<T>(monitor: &mut Monitor, wanted: impl Fn(&Change) -> Option<T>) -> T {
    loop {
        poll(&mut [PollFd::new(monitor, PollFlags::IN)], None).unwrap();
        let mut found = None;
        monitor.read(|change| found = found.take().or_else(|| wanted(&change)));
        if let Some(found) = found {
            return found;
        }
    }
}

fn arrival_named(name: &'static str) -> impl Fn(&Change) -> Option<Found> {
    move |change| match change {
        Change::Arrived(found) if found.identifiers.name == name => Some(found.clone()),
        _ => None,
    }
}

fn identifiers() -> Identifiers {
    Identifiers {
        name: "A Pad".to_string(),
        bus: 3,
        vendor: 0x054c,
        product: 0x0ce6,
        version: 0x8111,
        address: Some("0c:27:56:4f:bd:14".to_string()),
        serial: Some("7EED821FFB17".to_string()),
        port: Some("usb-0000:00:14.0-3/input0".to_string()),
    }
}

#[test]
fn a_pad_is_named_by_the_most_specific_thing_it_reports() {
    let all = identifiers();
    assert_eq!(all.id().to_string(), "address:0c:27:56:4f:bd:14");
    let no_address = Identifiers {
        address: None,
        ..all
    };
    assert_eq!(no_address.id().to_string(), "serial:054c:0ce6:7EED821FFB17");
    let no_serial = Identifiers {
        serial: None,
        ..no_address
    };
    assert_eq!(
        no_serial.id().to_string(),
        "port:054c:0ce6:usb-0000:00:14.0-3/input0"
    );
    let nothing = Identifiers {
        port: None,
        ..no_serial
    };
    assert_eq!(
        nothing.id(),
        PadId::Model(0x054c, 0x0ce6, "A Pad".to_string())
    );
    assert_eq!(nothing.id().to_string(), "model:054c:0ce6:A Pad");
}

#[test]
fn a_pad_is_seen_arriving_listed_while_connected_and_seen_leaving() {
    if !can_make_pads("a_pad_is_seen_arriving_listed_while_connected_and_seen_leaving") {
        return;
    }
    let mut monitor = Monitor::new().unwrap();
    let fake = FakePad::new("Hotplug Test Arrives", 0x1209, 0x5eed).unwrap();
    let arrived = wait_for(&mut monitor, arrival_named("Hotplug Test Arrives"));
    assert_eq!(arrived.node, fake.event_node());
    assert_eq!(
        (arrived.identifiers.vendor, arrived.identifiers.product),
        (0x1209, 0x5eed)
    );
    assert!(!arrived.is_own_output());

    // Read without opening, it is what opening it reports.
    let opened = read(&fake);
    assert_eq!(arrived.identifiers, *opened.identifiers());
    let listed = connected().unwrap();
    assert!(listed.contains(&arrived), "{listed:?}");

    let node = fake.event_node().to_path_buf();
    drop((fake, opened));
    wait_for(&mut monitor, |change| {
        (*change == Change::Left(node.clone())).then_some(())
    });
    // By name: another test's pad may already have the node's number.
    let still = connected().unwrap();
    assert!(
        !still
            .iter()
            .any(|found| found.identifiers.name == "Hotplug Test Arrives"),
        "{still:?}"
    );
}

#[test]
fn our_own_virtual_pads_are_listed_as_our_own() {
    if !can_make_pads("our_own_virtual_pads_are_listed_as_our_own") {
        return;
    }
    let (virtual_pad, _reader) = sink();
    let listed = connected().unwrap();
    let ours = listed
        .iter()
        .find(|found| found.node == virtual_pad.event_node())
        .unwrap();
    assert!(ours.is_own_output());
}

#[test]
fn a_pad_that_returns_takes_its_place_again_with_what_it_holds() {
    if !can_make_pads("a_pad_that_returns_takes_its_place_again_with_what_it_holds") {
        return;
    }
    let mut monitor = Monitor::new().unwrap();
    let primary = FakePad::new("Hotplug Test Returns", 0x1209, 0x5eed).unwrap();
    let assist = FakePad::new("Hotplug Test Assist", 0x1209, 0x5eed).unwrap();
    let (virtual_pad, mut game) = sink();
    let id = read(&primary).identifiers().id();
    let setup = Setup::new(
        mux(Mode::Priority),
        vec![Some(read(&primary)), Some(read(&assist))],
        vec![virtual_pad],
    );
    let (thread, reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();

    drop(primary);
    assert!(matches!(
        reports.recv().unwrap(),
        Report::PadGone {
            source: PRIMARY,
            ..
        }
    ));

    // The same pad comes back, already holding a button.
    let mut primary = FakePad::new("Hotplug Test Returns", 0x1209, 0x5eed).unwrap();
    primary.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
    // The monitor still holds the first arrival, under the same name: a
    // pad is known to be back when a node that exists now carries it.
    let node = primary.event_node().to_path_buf();
    let back = wait_for(&mut monitor, |change| match change {
        Change::Arrived(found) if found.identifiers.id() == id && found.node == node => {
            Some(found.clone())
        }
        _ => None,
    });
    assert_eq!(back.identifiers.name, "Hotplug Test Returns");
    let pad = Box::new(read(&primary));
    thread
        .send(Command::PadBack {
            setup: ID,
            source: PRIMARY,
            pad,
        })
        .unwrap();

    let held = PadState::NEUTRAL.with(gamepad::SOUTH, 1.0);
    let frames = read_until(&mut game, |frames| frames.last() == Some(&held));
    assert_eq!(last(&frames), held, "the held button arrives with the pad");
    primary
        .frame(&[(EV_KEY, BTN_SOUTH, 0), (EV_KEY, BTN_EAST, 1)])
        .unwrap();
    let after = PadState::NEUTRAL.with(gamepad::EAST, 1.0);
    let frames = read_until(&mut game, |frames| frames.last() == Some(&after));
    assert_eq!(last(&frames), after, "and it is read from then on");
}

#[test]
fn a_returning_pad_with_no_empty_place_is_handed_back() {
    if !can_make_pads("a_returning_pad_with_no_empty_place_is_handed_back") {
        return;
    }
    let primary = FakePad::new("Hotplug Test Primary", 0x1209, 0x5eed).unwrap();
    let assist = FakePad::new("Hotplug Test Assist", 0x1209, 0x5eed).unwrap();
    let (virtual_pad, _game) = sink();
    let setup = Setup::new(
        mux(Mode::Priority),
        vec![Some(read(&primary)), Some(read(&assist))],
        vec![virtual_pad],
    );
    let (thread, reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();

    // Its place is taken, then the setup isn't one that is running.
    for setup in [ID, ctrlassist_device::SetupId(99)] {
        let pad = Box::new(read(&primary));
        thread
            .send(Command::PadBack {
                setup,
                source: PRIMARY,
                pad,
            })
            .unwrap();
        match reports.recv().unwrap() {
            Report::PadRefused {
                setup: refused,
                source,
                ..
            } => assert_eq!((refused, source), (setup, PRIMARY)),
            other => unreachable!("expected the pad back, got {other:?}"),
        }
    }
}
