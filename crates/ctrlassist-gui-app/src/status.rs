//! How a profile is, in a word: what the Profiles list says under its
//! name, and what a pad's detail says beside each profile that names
//! the pad. One place, so the two can't disagree.

use ctrlassist::{Pad, Phase, Profile, Setup};
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

/// The pad `wanted` means for the profile `profile`, as a start would
/// find it: of the known pads it names, one that profile's setup has,
/// else one connected and in no setup, else one connected, else any.
/// For a pad named by its model several may fit, and this is the one
/// [`of`] judges the profile by.
#[must_use]
pub fn meant<'a>(wanted: &PadRef, pads: &'a [Pad], profile: &str) -> Option<&'a Pad> {
    let named = || pads.iter().filter(|pad| names(wanted, pad));
    named()
        .find(|pad| pad.connected && pad.setup.as_deref() == Some(profile))
        .or_else(|| named().find(|pad| pad.connected && pad.setup.is_none()))
        .or_else(|| named().find(|pad| pad.connected))
        .or_else(|| named().next())
}

/// The profile worked out, and whether its pads are still to choose.
/// The daemon keeps a preset that lacks only its pads, as not set up;
/// it is worked out here with stand-ins in their places, which are then
/// left out, so that the pads it does name can be told. Nothing for a
/// profile the daemon finds wrong.
#[must_use]
pub fn plan_of(profile: &Profile) -> Option<(Plan, bool)> {
    if let Ok(plan) = ctrlassist_protocol::profile::Profile::check(&profile.toml) {
        return Some((plan, false));
    }
    if profile.unset.is_empty() {
        return None;
    }
    let mut table: toml::Table = profile.toml.parse().ok()?;
    for (at, key) in profile.unset.iter().enumerate() {
        table.insert(
            key.clone(),
            toml::Value::String(format!("model:0000:{:04x}", at + 1)),
        );
    }
    let mut plan = ctrlassist_protocol::profile::Profile::check(&table.to_string()).ok()?;
    plan.sources
        .retain(|source| !profile.unset.contains(&source.name));
    Some((plan, true))
}

/// How the profile `name` is. `plan` is the profile worked out, or
/// nothing for one the daemon finds wrong; `unset`, that its pads are
/// still to choose.
///
/// A profile that runs says its setup's phase. One that doesn't is not
/// set up while its pads are still to choose, ready when every pad it
/// names is connected and in no setup, and waits for a pad otherwise.
#[must_use]
pub fn of(name: &str, plan: Option<&Plan>, unset: bool, setups: &[Setup], pads: &[Pad]) -> Word {
    if let Some(setup) = setups.iter().find(|setup| setup.name == name) {
        return match setup.phase {
            Phase::Running => ("Running", "success"),
            Phase::Held => ("Held", "warn"),
            Phase::Failed => ("Failed", "danger"),
            _ => ("Starting", "info"),
        };
    }
    if unset {
        return ("Not set up", "neutral");
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
        ("Waiting", "neutral")
    }
}

/// A source's name as a row says it: `primary` as "Primary".
#[must_use]
pub fn capitalized(name: &str) -> String {
    let mut letters = name.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

/// An identity as a sentence says it: `dualsense` as "DualSense".
#[must_use]
pub fn identity(id: &str) -> &str {
    match id {
        "xbox" => "Xbox",
        "dualsense" => "DualSense",
        other => other,
    }
}
