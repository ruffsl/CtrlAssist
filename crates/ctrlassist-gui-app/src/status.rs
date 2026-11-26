//! How a profile is, in a word: what the Profiles list says under its
//! name, and what a pad's detail says beside each profile that names
//! the pad. One place, so the two can't disagree.

use std::collections::BTreeMap;

use ctrlassist::{Pad, Phase, Profile, Setup};
use ctrlassist_protocol::profile::{PadRef, Plan};

/// How a profile is. Logic goes by this, and only [`word`](Self::word)
/// is shown, so a translated word can't change what the window does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Its setup is on its way to running.
    Starting,
    /// Its setup runs, with its virtual pads there.
    Running,
    /// Its setup waits for a pad that left.
    Held,
    /// Its setup stopped working; Stop clears it.
    Failed,
    /// Its pads are still to choose.
    NotSetUp,
    /// The daemon can't run it as it is written.
    Wrong,
    /// Every pad it names is connected and in no setup.
    Ready,
    /// A pad it names is away, or in a setup.
    Waiting,
    /// A change the window holds isn't a profile yet.
    Unfinished,
    /// Something asked of it from the window is still on its way: a
    /// change, a start, a copy or a delete.
    Saving,
    /// A change to it was kept, and the setup running it is taking it
    /// up, which may be a restart.
    Applying,
}

impl Status {
    /// The word a person reads.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Starting => "Starting",
            Self::Running => "Running",
            Self::Held => "Held",
            Self::Failed => "Failed",
            Self::NotSetUp => "Not set up",
            Self::Wrong => "Wrong",
            Self::Ready => "Ready",
            Self::Waiting => "Waiting",
            Self::Unfinished => "Unfinished",
            Self::Saving => "Saving…",
            Self::Applying => "Applying…",
        }
    }

    /// The tone it is shown in: a chip's tones, with `neutral` for the
    /// faint one.
    #[must_use]
    pub fn tone(self) -> &'static str {
        match self {
            Self::Running => "success",
            Self::Held | Self::Unfinished => "warn",
            Self::Failed | Self::Wrong => "danger",
            Self::Starting | Self::Ready => "info",
            Self::NotSetUp | Self::Waiting | Self::Saving | Self::Applying => "neutral",
        }
    }
}

/// Whether `pad` is one `wanted` means.
#[must_use]
pub fn names(wanted: &PadRef, pad: &Pad) -> bool {
    match wanted {
        PadRef::Exact(id) => *id == pad.id,
        PadRef::Model(vendor, product) => (*vendor, *product) == (pad.vendor, pad.product),
    }
}

/// What a profile's value for a pad names: a known pad by its id as it
/// stands, or what the value says in any of the ways a profile names one.
#[must_use]
pub fn wanted(value: &str, pads: &[Pad]) -> Option<PadRef> {
    if pads.iter().any(|pad| pad.id == value) {
        return Some(PadRef::Exact(value.to_string()));
    }
    PadRef::parse(value)
}

/// The pads `parts` name exactly: each is its part's, and no part named
/// by model takes it.
fn named_exactly<'a>(parts: &[(&str, &'a PadRef)]) -> Vec<&'a str> {
    parts
        .iter()
        .filter_map(|(_, wanted)| match wanted {
            PadRef::Exact(id) => Some(id.as_str()),
            PadRef::Model(..) => None,
        })
        .collect()
}

/// The pads a profile's `parts`, each a part's name and the pad it
/// names, would have if it started now, by part, as the daemon binds
/// them, in two passes: first each part named by its id has that pad, if
/// it is known; then each part named by model, in the order of the
/// parts' names, has the first pad of the model that is connected, in no
/// setup, named by its id by no part, and not taken by a part before it.
/// A part with no pad to have isn't there. The daemon takes the first of
/// a model to have connected, which it doesn't publish: the order here
/// is the one it publishes the pads in, by id. A pad of the setup `own`
/// counts as free, as a restart of that setup frees it. The tray binds
/// them the same way.
#[must_use]
pub fn bind<'a>(
    pads: &'a [Pad],
    parts: &[(&str, &PadRef)],
    own: Option<&str>,
) -> BTreeMap<String, &'a Pad> {
    let named = named_exactly(parts);
    let mut parts = parts.to_vec();
    parts.sort_by_key(|(name, _)| *name);
    let mut bound: BTreeMap<String, &Pad> = BTreeMap::new();
    for (name, wanted) in &parts {
        if let PadRef::Exact(id) = wanted
            && let Some(pad) = pads.iter().find(|pad| pad.id == *id)
        {
            bound.insert((*name).to_string(), pad);
        }
    }
    for (name, wanted) in parts {
        if matches!(wanted, PadRef::Exact(_)) {
            continue;
        }
        let pad = pads.iter().find(|pad| {
            names(wanted, pad)
                && pad.connected
                && (pad.setup.is_none() || pad.setup.as_deref() == own)
                && !named.contains(&pad.id.as_str())
                && !bound.values().any(|taken| taken.id == pad.id)
        });
        if let Some(pad) = pad {
            bound.insert(name.to_string(), pad);
        }
    }
    bound
}

/// The pad each of the profile `profile`'s `parts` has, by part: one
/// named by model, while `setup` runs the profile, the pad the setup has
/// for it unless another part names that pad by its id; otherwise the
/// pad a start would take now, as [`bind`] has it.
#[must_use]
pub fn taken<'a>(
    pads: &'a [Pad],
    parts: &[(&str, &PadRef)],
    setup: Option<&Setup>,
) -> BTreeMap<String, &'a Pad> {
    let mut taken = bind(pads, parts, None);
    let Some(setup) = setup else {
        return taken;
    };
    let named = named_exactly(parts);
    for (name, wanted) in parts {
        let has = setup
            .sources
            .iter()
            .find(|source| source.name == *name)
            .and_then(|source| source.pad.as_deref())
            .and_then(|id| pads.iter().find(|pad| pad.id == id))
            .filter(|pad| names(wanted, pad) && !named.contains(&pad.id.as_str()));
        if let (PadRef::Model(..), Some(pad)) = (wanted, has) {
            taken.insert((*name).to_string(), pad);
        }
    }
    taken
}

/// The pad each of a profile's `parts` means, by part, for showing: the
/// pad it has, as [`taken`] says, and for one named by model that has
/// none, a pad of the model that is connected, or any that is known.
#[must_use]
pub fn meant<'a>(
    pads: &'a [Pad],
    parts: &[(&str, &PadRef)],
    setup: Option<&Setup>,
) -> BTreeMap<String, &'a Pad> {
    let mut meant = taken(pads, parts, setup);
    for (name, wanted) in parts {
        if meant.contains_key(*name) || !matches!(wanted, PadRef::Model(..)) {
            continue;
        }
        let mut fitting = pads.iter().filter(|pad| names(wanted, pad));
        let first = fitting.clone().next();
        if let Some(pad) = fitting.find(|pad| pad.connected).or(first) {
            meant.insert((*name).to_string(), pad);
        }
    }
    meant
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
pub fn of(name: &str, plan: Option<&Plan>, unset: bool, setups: &[Setup], pads: &[Pad]) -> Status {
    if let Some(setup) = setups.iter().find(|setup| setup.name == name) {
        return match setup.phase {
            Phase::Running => Status::Running,
            Phase::Held => Status::Held,
            Phase::Failed => Status::Failed,
            // Starting, and any phase a later daemon adds.
            _ => Status::Starting,
        };
    }
    if unset {
        return Status::NotSetUp;
    }
    let Some(plan) = plan else {
        return Status::Wrong;
    };
    // Each part a pad of its own, as a start would bind them: two parts
    // named by one model need two pads of it.
    let parts: Vec<(&str, &PadRef)> = plan
        .sources
        .iter()
        .map(|source| (source.name.as_str(), &source.pad))
        .collect();
    let bound = bind(pads, &parts, None);
    let free = |name: &str| {
        bound
            .get(name)
            .is_some_and(|pad| pad.connected && pad.setup.is_none())
    };
    if parts.iter().all(|(name, _)| free(name)) {
        Status::Ready
    } else {
        Status::Waiting
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
        "steam-controller" => "Steam Controller",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(id: &str, setup: Option<&str>) -> Pad {
        Pad {
            id: id.to_string(),
            name: id.to_string(),
            vendor: 0x054c,
            product: 0x0ce6,
            connected: true,
            hidden: false,
            setup: setup.map(str::to_string),
            rumble: Some(true),
            bus: None,
        }
    }

    /// Two parts named by one model are two pads of it: ready with two
    /// free, waiting with one, as the tray says it too.
    #[test]
    fn two_parts_of_one_model_need_two_free_pads_to_be_ready() {
        let toml =
            "mode = \"toggle\"\nprimary = \"model:054c:0ce6\"\nassist = \"model:054c:0ce6\"\n";
        let plan = ctrlassist_protocol::profile::Profile::check(toml).expect("a profile");
        let one = [pad("address:aa", None), pad("address:bb", Some("desk"))];
        assert_eq!(of("couch", Some(&plan), false, &[], &one), Status::Waiting);
        let two = [pad("address:aa", None), pad("address:bb", None)];
        assert_eq!(of("couch", Some(&plan), false, &[], &two), Status::Ready);
        // Bound in the order of the parts' names, each its own pad.
        let parts: Vec<(&str, &PadRef)> = plan
            .sources
            .iter()
            .map(|source| (source.name.as_str(), &source.pad))
            .collect();
        let bound = bind(&two, &parts, None);
        assert_eq!(bound["assist"].id, "address:aa");
        assert_eq!(bound["primary"].id, "address:bb");
    }

    /// A part named by model takes no pad another part names by its id,
    /// though it comes first in order: the daemon binds the parts named
    /// by id first. So the profile is ready with a second pad of the
    /// model, and waits without one.
    #[test]
    fn a_part_named_by_model_leaves_the_pads_named_by_id() {
        let toml =
            "mode = \"toggle\"\nassist = \"model:054c:0ce6\"\nprimary = \"serial:054c:0ce6:aa\"\n";
        let plan = ctrlassist_protocol::profile::Profile::check(toml).expect("a profile");
        let two = [
            pad("serial:054c:0ce6:aa", None),
            pad("serial:054c:0ce6:bb", None),
        ];
        let parts: Vec<(&str, &PadRef)> = plan
            .sources
            .iter()
            .map(|source| (source.name.as_str(), &source.pad))
            .collect();
        let bound = bind(&two, &parts, None);
        assert_eq!(bound["assist"].id, "serial:054c:0ce6:bb");
        assert_eq!(bound["primary"].id, "serial:054c:0ce6:aa");
        assert_eq!(of("couch", Some(&plan), false, &[], &two), Status::Ready);
        let one = [pad("serial:054c:0ce6:aa", None)];
        assert!(!bind(&one, &parts, None).contains_key("assist"));
        assert_eq!(of("couch", Some(&plan), false, &[], &one), Status::Waiting);
    }

    /// While a setup runs, a part named by model shows the pad the setup
    /// has for it, but never one another part names by its id: taking
    /// the change, the daemon gives that pad up.
    #[test]
    fn a_running_part_named_by_model_gives_up_a_pad_named_by_id() {
        let model = PadRef::Model(0x054c, 0x0ce6);
        let exact = PadRef::Exact("serial:054c:0ce6:aa".to_string());
        let parts = [("assist", &model), ("primary", &exact)];
        let pads = [
            pad("serial:054c:0ce6:aa", Some("couch")),
            pad("serial:054c:0ce6:bb", Some("couch")),
        ];
        let setup = Setup {
            name: "couch".to_string(),
            start: 1,
            phase: Phase::Running,
            reason: None,
            sources: [
                ("assist", "serial:054c:0ce6:aa"),
                ("primary", "serial:054c:0ce6:bb"),
            ]
            .into_iter()
            .map(|(name, pad)| ctrlassist::SetupSource {
                name: name.to_string(),
                pad: Some(pad.to_string()),
                present: true,
            })
            .collect(),
            sinks: Vec::new(),
            nodes: Vec::new(),
            stale: false,
        };
        let taken = taken(&pads, &parts, Some(&setup));
        assert!(!taken.contains_key("assist"), "{taken:?}");
        assert_eq!(taken["primary"].id, "serial:054c:0ce6:aa");
    }
}
