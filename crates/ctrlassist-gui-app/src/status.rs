//! How a profile is, in a word: what the Profiles list says under its
//! name, and what a pad's detail says beside each profile that names
//! the pad. One place, so the two can't disagree.

use ctrlassist::{Pad, Phase, Setup};
use ctrlassist_protocol::profile::{PadRef, Plan};

/// A word and the tone it is shown in: a chip's tones, with `neutral`
/// for the faint one.
pub type Word = (&'static str, &'static str);

/// Whether `pad` is one `wanted` means.
#[must_use]
pub fn names(wanted: &PadRef, pad: &Pad) -> bool {
    match wanted {
        PadRef::Exact(id) => *id == pad.id,
        PadRef::Model(vendor, product) => (*vendor, *product) == (pad.vendor, pad.product),
    }
}

/// How the profile `name` is. `plan` is the profile worked out, or
/// nothing for one the daemon finds wrong.
///
/// A profile that runs says its setup's phase. One that doesn't is
/// ready when every pad it names is connected and in no setup, and
/// waits for a pad otherwise.
#[must_use]
pub fn of(name: &str, plan: Option<&Plan>, setups: &[Setup], pads: &[Pad]) -> Word {
    if let Some(setup) = setups.iter().find(|setup| setup.name == name) {
        return match setup.phase {
            Phase::Running => ("Running", "success"),
            Phase::Held => ("Held", "warn"),
            Phase::Failed => ("Failed", "danger"),
            _ => ("Starting", "info"),
        };
    }
    let Some(plan) = plan else {
        return ("Wrong", "danger");
    };
    let free = |wanted: &PadRef| {
        pads.iter()
            .any(|pad| names(wanted, pad) && pad.connected && pad.setup.is_none())
    };
    if plan.sources.iter().all(|source| free(&source.pad)) {
        ("Ready", "info")
    } else {
        ("Waits for a pad", "neutral")
    }
}
