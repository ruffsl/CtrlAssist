//! What a caller names things by.
//!
//! Pads, virtual pads and nodes are all positions in a graph's lists, and
//! a position is just a number. Giving each kind its own type means a
//! virtual pad's number can't be passed where a pad's is wanted.

/// A pad, by its position in the graph's sources.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SourceId(pub usize);

/// A virtual pad, by its position in the graph's sinks.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct SinkId(pub usize);

/// A node, by its position in the graph's nodes.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NodeId(pub usize);

/// A set of pads, such as the ones a virtual pad's feedback goes to.
///
/// It holds the first 32 pads of a graph, which is as many as a graph may
/// have; a pad beyond that is never a member.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct SourceSet(u32);

impl SourceSet {
    /// No pads.
    pub const EMPTY: Self = Self(0);
    /// As many pads as a set can hold.
    pub const CAPACITY: usize = 32;

    fn bit(source: SourceId) -> u32 {
        if source.0 < Self::CAPACITY {
            1 << source.0
        } else {
            0
        }
    }

    /// The set of the given pads.
    #[must_use]
    pub fn of(sources: &[SourceId]) -> Self {
        Self(
            sources
                .iter()
                .fold(0, |bits, source| bits | Self::bit(*source)),
        )
    }

    /// Whether a pad is in the set.
    #[must_use]
    pub fn contains(self, source: SourceId) -> bool {
        self.0 & Self::bit(source) != 0
    }

    /// Whether the set has no pads.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The pads in the set, in order.
    pub fn iter(self) -> impl Iterator<Item = SourceId> {
        (0..Self::CAPACITY)
            .map(SourceId)
            .filter(move |source| self.contains(*source))
    }
}

impl std::ops::BitOr for SourceSet {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl std::ops::BitOrAssign for SourceSet {
    fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

impl From<SourceId> for SourceSet {
    fn from(source: SourceId) -> Self {
        Self(Self::bit(source))
    }
}
