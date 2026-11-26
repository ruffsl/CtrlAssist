//! The test crate's fake Steam Controller builds its reports from its
//! own reading of the layout, apart from this module's. Here each report
//! it builds is read back by this module, field by field, so that the two
//! readings check each other: a place one of them has wrong shows here,
//! with no device needed.

use super::*;
use ctrlassist_test::{
    ChargeState, StateReport, SteamBattery, SteamButton, SteamState, Trackpad as FakeTrackpad,
    wireless_report,
};

/// A state with a different value in every field, so that a field read
/// from another's place can't pass.
fn every_field() -> SteamState {
    SteamState {
        left_trigger: 1111,
        right_trigger: 2222,
        left_stick: [-3333, 4444],
        right_stick: [5555, -6666],
        left_pad: FakeTrackpad {
            touched: true,
            x: -7777,
            y: 8888,
            pressure: 9999,
        },
        right_pad: FakeTrackpad {
            touched: false,
            x: 10101,
            y: -12121,
            pressure: 13131,
        },
        sensor_clock: 0x0123_4567,
        trackpad_clock: 0x89ab,
        accel: [1414, -1515, 16161],
        gyro: [-1717, 1818, -1919],
        orientation: [2020, -2121, 2222, -2323],
        ..SteamState::default()
    }
    .with(SteamButton::South)
    .with(SteamButton::Menu)
    .with(SteamButton::UpperRightBack)
    .with(SteamButton::LowerLeftBack)
    .with(SteamButton::RightStickTouch)
    .with(SteamButton::LeftGripTouch)
}

/// The sample this module should read from `state` as report `kind`: the
/// fake's counts through this module's own conversions, which its unit
/// tests check apart from the layout.
fn expected(state: &SteamState, kind: StateReport, sequence: u8) -> Sample {
    let trackpad = |pad: FakeTrackpad| Trackpad {
        touched: pad.touched,
        x: across(pad.x),
        y: down(pad.y),
        pressure: pressure_of(pad.pressure),
    };
    let (gyro, accel) = motion(state.gyro, state.accel);
    let mut buttons = state.buttons;
    if state.left_pad.touched {
        buttons |= Button::LeftTrackpadTouch.mask();
    }
    if state.right_pad.touched {
        buttons |= Button::RightTrackpadTouch.mask();
    }
    let narrow =
        u16::try_from((state.sensor_clock / NARROW_TICK_US) & 0xffff).expect("held to 16 bits");
    Sample {
        sequence,
        buttons: Buttons(buttons),
        triggers: [trigger(state.left_trigger), trigger(state.right_trigger)],
        left_stick: stick(state.left_stick),
        right_stick: stick(state.right_stick),
        left_trackpad: trackpad(state.left_pad),
        right_trackpad: trackpad(state.right_pad),
        gyro,
        accel,
        time: match kind {
            StateReport::WithShortClocks => SensorTime::Narrow(narrow),
            _ => SensorTime::Wide(state.sensor_clock),
        },
        trackpad_time: (kind == StateReport::WithShortClocks).then_some(state.trackpad_clock),
        orientation: (kind == StateReport::Full).then_some(state.orientation),
    }
}

#[test]
fn every_field_of_the_fake_s_state_reports_is_read_where_it_wrote_it() {
    let state = every_field();
    for (kind, id) in [
        (StateReport::Full, STATE),
        (StateReport::WithoutOrientation, STATE_PLAIN),
        (StateReport::WithShortClocks, STATE_TIMESTAMPED),
    ] {
        let report = state.report(kind, 77);
        assert_eq!(report[0], id, "{kind:?}");
        let Some(Input::State(sample)) = parse(&report) else {
            panic!("{kind:?} wasn't read as a state: {report:02x?}");
        };
        assert_eq!(sample, expected(&state, kind, 77), "{kind:?}");
    }
}

/// The narrow clock is the fake's microseconds in ticks of 32, wrapping
/// at 16 bits, so two of its readings are as far apart in microseconds,
/// to the tick, as the fake's clock said.
#[test]
fn the_fake_s_narrow_clock_reads_back_as_its_microseconds() {
    let at = |sensor_clock: u32| {
        let state = SteamState {
            sensor_clock,
            ..SteamState::default()
        };
        let Some(Input::State(sample)) = parse(&state.report(StateReport::WithShortClocks, 0))
        else {
            panic!("not read as a state");
        };
        sample.time
    };
    // Across the 16-bit wrap: 2,097,152 µs is 65,536 ticks.
    let earlier = at(2_097_152 - 4000);
    let later = at(2_097_152 + 4000);
    assert_eq!(later.since(earlier), Some(8000));
}

#[test]
fn the_fake_s_battery_report_is_read_field_by_field() {
    for (charge, read) in [
        (ChargeState::Discharging, Charge::Discharging),
        (ChargeState::Charging, Charge::Charging),
        (ChargeState::Charged, Charge::Full),
    ] {
        let battery = SteamBattery {
            charge,
            level: 61,
            battery_millivolts: 3712,
            system_millivolts: 3301,
            input_millivolts: 5003,
            battery_milliamps: 412,
            input_milliamps: 913,
            temperature: 27_500,
        };
        assert_eq!(
            parse(&battery.report()),
            Some(Input::Battery(Battery {
                charge: read,
                percent: 61,
                battery_mv: 3712,
                system_mv: 3301,
                input_mv: 5003,
                battery_ma: 412,
                input_ma: 913,
                temperature_millidegrees: 27_500,
            }))
        );
    }
}

/// The fake's wireless events, as it sends them, are a pad joining and
/// leaving.
#[test]
fn the_fake_s_wireless_events_are_read_as_joining_and_leaving() {
    assert_eq!(
        parse(&wireless_report(true)),
        Some(Input::Wireless(Wireless::Connected))
    );
    assert_eq!(
        parse(&wireless_report(false)),
        Some(Input::Wireless(Wireless::Disconnected))
    );
}
