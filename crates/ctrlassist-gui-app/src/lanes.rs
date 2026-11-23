//! Lanes of work, one for each name: what is asked of one profile waits
//! for what was asked of it before, and for nothing else, so that an
//! answer that takes long holds up nothing but what has to come after it.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, VecDeque};

/// The lanes with something under way, each with what waits behind it.
#[derive(Debug)]
pub(crate) struct Lanes<T> {
    busy: BTreeMap<String, VecDeque<T>>,
}

impl<T> Default for Lanes<T> {
    fn default() -> Self {
        Self {
            busy: BTreeMap::new(),
        }
    }
}

impl<T> Lanes<T> {
    /// Takes `item` onto `lane`: handed back to be started now when
    /// nothing is under way there, and queued behind what is otherwise.
    pub(crate) fn take(&mut self, lane: &str, item: T) -> Option<T> {
        match self.busy.entry(lane.to_string()) {
            Entry::Occupied(mut waiting) => {
                waiting.get_mut().push_back(item);
                None
            }
            Entry::Vacant(idle) => {
                idle.insert(VecDeque::new());
                Some(item)
            }
        }
    }

    /// What was under way on `lane` is done: the next to start there, or
    /// none, the lane then being idle. Of what waits, one that
    /// `carried(it, next)` says the one after it carries whole is passed
    /// over, and handed back first, to be settled without being done.
    pub(crate) fn done(
        &mut self,
        lane: &str,
        carried: impl Fn(&T, &T) -> bool,
    ) -> (Vec<T>, Option<T>) {
        let Some(waiting) = self.busy.get_mut(lane) else {
            return (Vec::new(), None);
        };
        let mut passed = Vec::new();
        while let Some(next) = waiting.pop_front() {
            if waiting.front().is_some_and(|after| carried(&next, after)) {
                passed.push(next);
                continue;
            }
            return (passed, Some(next));
        }
        self.busy.remove(lane);
        (passed, None)
    }

    #[cfg(test)]
    /// Whether anything is under way or waiting.
    pub(crate) fn is_idle(&self) -> bool {
        self.busy.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Even numbers stand for changes, each carrying the one before it
    /// when that is a change too; odd ones for anything else.
    fn carried(a: &u32, b: &u32) -> bool {
        a.is_multiple_of(2) && b.is_multiple_of(2)
    }

    #[test]
    fn a_lane_keeps_its_order_holds_no_other_and_passes_over_what_is_carried() {
        let mut lanes = Lanes::default();
        assert_eq!(lanes.take("couch", 2), Some(2));
        assert_eq!(lanes.take("couch", 4), None);
        assert_eq!(lanes.take("couch", 6), None);
        assert_eq!(lanes.take("couch", 7), None);
        assert_eq!(lanes.take("couch", 8), None);
        // Another lane doesn't wait.
        assert_eq!(lanes.take("desk", 10), Some(10));
        // 4 is carried by 6; the delete, 7, isn't passed over, nor is
        // the change after it, 8, carried by anything before.
        assert_eq!(lanes.done("couch", carried), (vec![4], Some(6)));
        assert_eq!(lanes.done("couch", carried), (vec![], Some(7)));
        assert_eq!(lanes.done("couch", carried), (vec![], Some(8)));
        assert_eq!(lanes.done("couch", carried), (vec![], None));
        assert!(!lanes.is_idle());
        assert_eq!(lanes.done("desk", carried), (vec![], None));
        assert!(lanes.is_idle());
        assert_eq!(lanes.done("desk", carried), (vec![], None));
        assert_eq!(lanes.take("couch", 12), Some(12));
    }
}
