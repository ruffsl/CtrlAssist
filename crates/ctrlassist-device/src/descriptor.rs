//! Checking a HID report descriptor as the kernel will parse it.
//!
//! A virtual DualSense is made from a descriptor that may have been
//! copied from a pad and kept on disk. If the kernel's parser refuses
//! it, the PlayStation driver's probe fails before the device is
//! started: no driver binds, uhid sends nothing, and no event would ever
//! end the wait for the pad. So a descriptor is checked before it is
//! used, by the rules `hid_open_report` applies (drivers/hid/hid-core.c:
//! `fetch_item`, `hid_parser_main`, `hid_parser_global`,
//! `hid_parser_local`, `hid_add_field`), and more strictly where a rule
//! is in doubt: a copy refused here leaves the shipped descriptor in
//! use, where one the kernel refuses would hang.

#![warn(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::string_slice,
    clippy::cast_possible_truncation
)]

/// The most a report descriptor holds: the kernel's
/// `HID_MAX_DESCRIPTOR_SIZE`, and uhid's limit.
pub(crate) const MAX_SIZE: usize = 4096;
/// `HID_GLOBAL_STACK_SIZE`.
const GLOBAL_STACK: usize = 4;
/// `HID_MAX_USAGES`.
const MAX_USAGES: u32 = 12288;
/// `HID_MAX_IDS`.
const MAX_IDS: usize = 256;
/// The largest report size in bits a field may have.
const MAX_REPORT_SIZE: u32 = 256;
/// The most bits one report may hold: uhid's `max_buffer_size`, less the
/// report's number.
const MAX_REPORT_BITS: u64 = (4096 - 1) * 8;

/// The reports a DualSense exchanges with the kernel's driver over USB,
/// by kind and number: its input report, its output report, and the
/// calibration, pairing and firmware feature reports.
const DUALSENSE_REPORTS: [(Kind, u8); 5] = [
    (Kind::Input, 0x01),
    (Kind::Output, 0x02),
    (Kind::Feature, 0x05),
    (Kind::Feature, 0x09),
    (Kind::Feature, 0x20),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Input,
    Output,
    Feature,
}

impl Kind {
    fn index(self) -> usize {
        match self {
            Self::Input => 0,
            Self::Output => 1,
            Self::Feature => 2,
        }
    }
}

/// The global items in force.
#[derive(Clone, Copy, Default)]
struct Global {
    logical_minimum: i32,
    logical_maximum: i32,
    report_size: u32,
    report_count: u32,
    report_id: u32,
}

/// The local items in force, which each main item clears.
#[derive(Clone, Copy, Default)]
struct Local {
    usages: u32,
    usage_minimum: u32,
    delimiter_depth: u32,
    delimiter_branch: u32,
}

/// The reports declared so far: their length in bits, by kind and
/// number, and whether each is declared at all.
struct Reports {
    bits: [[u64; MAX_IDS]; 3],
    declared: [[bool; MAX_IDS]; 3],
}

impl Reports {
    fn declares(&self, kind: Kind, id: u8) -> bool {
        self.declared
            .get(kind.index())
            .and_then(|declared| declared.get(usize::from(id)))
            .copied()
            .unwrap_or(false)
    }
}

/// An item's data as unsigned, as `item_udata` reads it.
fn unsigned(data: &[u8]) -> u32 {
    match *data {
        [a] => u32::from(a),
        [a, b] => u32::from(u16::from_le_bytes([a, b])),
        [a, b, c, d] => u32::from_le_bytes([a, b, c, d]),
        _ => 0,
    }
}

/// An item's data as signed, as `item_sdata` reads it.
fn signed(data: &[u8]) -> i32 {
    match *data {
        [a] => i32::from(i8::from_le_bytes([a])),
        [a, b] => i32::from(i16::from_le_bytes([a, b])),
        [a, b, c, d] => i32::from_le_bytes([a, b, c, d]),
        _ => 0,
    }
}

/// Whether the kernel's parser takes `descriptor`, and it declares the
/// reports a DualSense exchanges with the kernel's driver.
pub(crate) fn is_dualsense(descriptor: &[u8]) -> bool {
    parse(descriptor).is_some_and(|reports| {
        DUALSENSE_REPORTS
            .iter()
            .all(|(kind, id)| reports.declares(*kind, *id))
    })
}

/// The reports a descriptor declares, or `None` where the kernel's parser
/// would refuse it, or might.
fn parse(descriptor: &[u8]) -> Option<Box<Reports>> {
    if descriptor.is_empty() || descriptor.len() > MAX_SIZE {
        return None;
    }
    let mut reports = Box::new(Reports {
        bits: [[0; MAX_IDS]; 3],
        declared: [[false; MAX_IDS]; 3],
    });
    let mut global = Global::default();
    let mut stack = [Global::default(); GLOBAL_STACK];
    let mut pushed = 0usize;
    let mut local = Local::default();
    let mut collections = 0usize;
    let mut rest = descriptor;
    while let Some((&prefix, after)) = rest.split_first() {
        let tag = prefix >> 4;
        let kind = (prefix >> 2) & 0x03;
        // A long item: the kernel takes only short ones.
        if tag == 0x0f {
            return None;
        }
        let size = match prefix & 0x03 {
            3 => 4,
            size => usize::from(size),
        };
        let (data, after) = after.split_at_checked(size)?;
        rest = after;
        match kind {
            0 => {
                match tag {
                    // Input, output and feature.
                    0x08 => add_field(&mut reports, &global, Kind::Input)?,
                    0x09 => add_field(&mut reports, &global, Kind::Output)?,
                    0x0b => add_field(&mut reports, &global, Kind::Feature)?,
                    0x0a => collections = collections.checked_add(1)?,
                    0x0c => collections = collections.checked_sub(1)?,
                    // Anything else is only warned of.
                    _ => {}
                }
                local = Local::default();
            }
            1 => match tag {
                // Usage page, physical extent, unit exponent and unit.
                0x00 | 0x03..=0x06 => {}
                0x01 => global.logical_minimum = signed(data),
                0x02 => {
                    global.logical_maximum = if global.logical_minimum < 0 {
                        signed(data)
                    } else {
                        unsigned(data).cast_signed()
                    };
                }
                0x07 => {
                    global.report_size = unsigned(data);
                    if global.report_size > MAX_REPORT_SIZE {
                        return None;
                    }
                }
                0x08 => {
                    global.report_id = unsigned(data);
                    if global.report_id == 0 || global.report_id as usize >= MAX_IDS {
                        return None;
                    }
                }
                0x09 => {
                    global.report_count = unsigned(data);
                    if global.report_count > MAX_USAGES {
                        return None;
                    }
                }
                0x0a => {
                    *stack.get_mut(pushed)? = global;
                    pushed = pushed.checked_add(1)?;
                }
                0x0b => {
                    pushed = pushed.checked_sub(1)?;
                    global = *stack.get(pushed)?;
                }
                _ => return None,
            },
            2 => {
                let alternative = local.delimiter_branch > 1;
                match tag {
                    0x00 if !alternative => {
                        local.usages = local.usages.checked_add(1)?;
                    }
                    0x01 if !alternative => local.usage_minimum = unsigned(data),
                    // Stricter than the kernel, which clamps a range that
                    // runs past the most usages, or backwards.
                    0x02 if !alternative => {
                        let count = unsigned(data)
                            .checked_sub(local.usage_minimum)?
                            .checked_add(1)?;
                        local.usages = local.usages.checked_add(count)?;
                    }
                    0x0a => match unsigned(data) {
                        1 if local.delimiter_depth == 0 => {
                            local.delimiter_depth = 1;
                            local.delimiter_branch = local.delimiter_branch.saturating_add(1);
                        }
                        0 if local.delimiter_depth == 1 => local.delimiter_depth = 0,
                        _ => return None,
                    },
                    _ => {}
                }
                if local.usages > MAX_USAGES {
                    return None;
                }
            }
            // Reserved items are skipped.
            _ => {}
        }
    }
    (collections == 0 && local.delimiter_depth == 0).then_some(reports)
}

/// Adds a main item's field to its report, as `hid_add_field` does.
fn add_field(reports: &mut Reports, global: &Global, kind: Kind) -> Option<()> {
    let (minimum, maximum) = (global.logical_minimum, global.logical_maximum);
    let backwards = if minimum < 0 {
        maximum < minimum
    } else {
        maximum.cast_unsigned() < minimum.cast_unsigned()
    };
    if backwards {
        return None;
    }
    let id = usize::try_from(global.report_id).ok()?;
    let bits = reports.bits.get_mut(kind.index())?.get_mut(id)?;
    let field = u64::from(global.report_size).checked_mul(u64::from(global.report_count))?;
    *bits = bits.checked_add(field)?;
    if *bits > MAX_REPORT_BITS {
        return None;
    }
    *reports.declared.get_mut(kind.index())?.get_mut(id)? = true;
    Some(())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn shipped() -> Vec<u8> {
        crate::DualSenseData::shipped().descriptor
    }

    /// The test crate's stand-in for a DualSense's descriptor: the five
    /// reports, as vendor-defined data.
    const STAND_IN: [u8; 55] = [
        0x06, 0x00, 0xff, 0x09, 0x01, 0xa1, 0x01, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x85,
        0x01, 0x95, 0x3f, 0x09, 0x02, 0x81, 0x02, 0x85, 0x02, 0x95, 0x3f, 0x09, 0x03, 0x91, 0x02,
        0x85, 0x05, 0x95, 0x28, 0x09, 0x04, 0xb1, 0x02, 0x85, 0x09, 0x95, 0x13, 0x09, 0x05, 0xb1,
        0x02, 0x85, 0x20, 0x95, 0x3f, 0x09, 0x06, 0xb1, 0x02, 0xc0,
    ];

    /// The stand-in with `items` in place of its last report.
    fn with(items: &[u8]) -> Vec<u8> {
        let mut descriptor = STAND_IN[..46].to_vec();
        descriptor.extend_from_slice(items);
        descriptor
    }

    #[test]
    fn a_dualsense_s_descriptor_and_the_tests_stand_in_are_taken() {
        assert!(is_dualsense(&shipped()));
        assert!(is_dualsense(&STAND_IN));
        // The firmware report again, as written above.
        assert!(is_dualsense(&with(&STAND_IN[46..])));
    }

    #[test]
    fn what_the_kernel_s_parser_refuses_is_refused() {
        let firmware = [0x85, 0x20, 0x95, 0x3f, 0x09, 0x06, 0xb1, 0x02];
        let refused: [(&str, Vec<u8>); 13] = [
            ("empty", Vec::new()),
            ("cut short", shipped()[..shipped().len() - 1].to_vec()),
            ("an item cut short", with(&[0x26, 0xff])),
            ("a long item", with(&[0xfe, 0x00, 0x00, 0xc0])),
            ("a collection left open", with(&firmware)),
            (
                "a collection closed twice",
                with(&[&firmware[..], &[0xc0, 0xc0]].concat()),
            ),
            ("report 0", with(&[0x85, 0x00, 0xc0])),
            ("a field too wide", with(&[0x76, 0x01, 0x01, 0xc0])),
            ("too many fields", with(&[0x96, 0x01, 0x30, 0xc0])),
            ("a pop with nothing pushed", with(&[0xb4, 0xc0])),
            (
                "pushed too deep",
                with(&[0xa4, 0xa4, 0xa4, 0xa4, 0xa4, 0xc0]),
            ),
            ("an unknown global", with(&[0xc4, 0xc0])),
            (
                "a report too long",
                with(&[0x85, 0x30, 0x96, 0x00, 0x10, 0xb1, 0x02, 0xc0]),
            ),
        ];
        for (what, descriptor) in refused {
            assert!(!is_dualsense(&descriptor), "{what}: {descriptor:02x?}");
        }
        // Taken by the parser, but without the firmware report the
        // driver reads.
        assert!(parse(&with(&[0xc0])).is_some());
        assert!(!is_dualsense(&with(&[0xc0])));
        // A field whose range runs backwards.
        let backwards = [&[0x15, 0x10, 0x25, 0x01][..], &firmware, &[0xc0]].concat();
        assert!(!is_dualsense(&with(&backwards)));
        // A delimiter left open, or closed without being opened.
        assert!(!is_dualsense(&with(
            &[&firmware[..], &[0xc0, 0xa9, 0x01]].concat()
        )));
        assert!(!is_dualsense(&with(
            &[&[0xa9, 0x00][..], &firmware, &[0xc0]].concat()
        )));
        // Pushed and popped is as it was.
        assert!(is_dualsense(&with(
            &[&[0xa4, 0xb4][..], &firmware, &[0xc0]].concat()
        )));
        // Too large to be made at all.
        let mut large = shipped();
        large.resize(MAX_SIZE + 1, 0);
        assert!(!is_dualsense(&large));
    }

    proptest! {
        /// No bytes can make the check panic.
        #[test]
        fn no_descriptor_can_panic_the_check(bytes in prop::collection::vec(any::<u8>(), 0..600)) {
            let _ = is_dualsense(&bytes);
        }

        /// Nor can a real descriptor with one byte changed.
        #[test]
        fn no_change_to_a_real_descriptor_can_panic_the_check(at in 0usize..273, byte in any::<u8>()) {
            let mut descriptor = shipped();
            if let Some(place) = descriptor.get_mut(at) {
                *place = byte;
            }
            let _ = is_dualsense(&descriptor);
        }
    }
}
