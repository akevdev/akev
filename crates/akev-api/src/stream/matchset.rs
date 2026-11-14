//! Per-connection server-side filter state: which exchange ids currently match the
//! connection's filter, and the `filtered: {add, remove}` deltas for each batch.
//!
//! Only exchanges that changed in a batch are re-evaluated (a status filter starts matching
//! once the response arrives; `is:pinned` flips on PATCH...), so the cost per batch is
//! proportional to the batch, not to the number of stored exchanges.

use akev_core::ExchangeId;
use serde::Serialize;
use std::collections::HashSet;

/// `filtered` payload of an `exchanges` batch.
#[derive(Debug, Default, Clone, PartialEq, Serialize)]
pub struct FilterDelta {
    pub add: Vec<ExchangeId>,
    pub remove: Vec<ExchangeId>,
}

/// The set of matching ids for one connection.
#[derive(Debug, Clone, PartialEq)]
pub enum MatchSet {
    /// The filter matches everything (`ids: null`).
    All,
    /// Explicit set of matching ids.
    Ids(HashSet<ExchangeId>),
}

impl MatchSet {
    pub fn from_ids(ids: impl IntoIterator<Item = ExchangeId>) -> Self {
        MatchSet::Ids(ids.into_iter().collect())
    }

    /// Sorted ids, or `None` when everything matches.
    pub fn sorted_ids(&self) -> Option<Vec<ExchangeId>> {
        match self {
            MatchSet::All => None,
            MatchSet::Ids(ids) => {
                let mut v: Vec<_> = ids.iter().copied().collect();
                v.sort_unstable();
                Some(v)
            }
        }
    }

    #[cfg(test)]
    pub fn contains(&self, id: ExchangeId) -> bool {
        match self {
            MatchSet::All => true,
            MatchSet::Ids(ids) => ids.contains(&id),
        }
    }

    /// Re-evaluate the exchanges of one batch. `added` are new to the client, `updated` are
    /// known ones that changed; `matches` evaluates the filter for one id.
    ///
    /// For a match-everything filter, every newly added id is reported in `add` (idempotent
    /// for clients that treat `ids: null` as "all").
    pub fn apply(
        &mut self,
        added: &[ExchangeId],
        updated: &[ExchangeId],
        mut matches: impl FnMut(ExchangeId) -> bool,
    ) -> FilterDelta {
        let mut delta = FilterDelta::default();
        match self {
            MatchSet::All => delta.add.extend_from_slice(added),
            MatchSet::Ids(set) => {
                for &id in added.iter().chain(updated) {
                    if matches(id) {
                        if set.insert(id) {
                            delta.add.push(id);
                        }
                    } else if set.remove(&id) {
                        delta.remove.push(id);
                    }
                }
            }
        }
        delta.add.sort_unstable();
        delta.add.dedup();
        delta.remove.sort_unstable();
        delta
    }

    /// Forget deleted exchanges (clients apply `removed` to their filtered set implicitly).
    pub fn remove(&mut self, ids: &[ExchangeId]) {
        if let MatchSet::Ids(set) = self {
            for id in ids {
                set.remove(id);
            }
        }
    }

    /// Everything was cleared.
    pub fn clear(&mut self) {
        if let MatchSet::Ids(set) = self {
            set.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[u64]) -> Vec<ExchangeId> {
        v.iter().map(|&i| ExchangeId(i)).collect()
    }

    #[test]
    fn delta_only_reports_transitions() {
        let mut m = MatchSet::from_ids(ids(&[1, 2]));
        // 3 new & matching, 4 new not matching, 2 updated still matching, 1 updated stops matching.
        let matching: HashSet<u64> = [2, 3].into();
        let d = m.apply(&ids(&[3, 4]), &ids(&[1, 2]), |id| matching.contains(&id.0));
        assert_eq!(d.add, ids(&[3]));
        assert_eq!(d.remove, ids(&[1]));
        assert_eq!(m.sorted_ids(), Some(ids(&[2, 3])));
        // Re-applying the same state yields an empty delta.
        let d = m.apply(&[], &ids(&[1, 2, 3, 4]), |id| matching.contains(&id.0));
        assert_eq!(d, FilterDelta::default());
    }

    #[test]
    fn status_filter_starts_matching_on_update() {
        // `status:200` doesn't match while the request is pending, then matches on response.
        let mut m = MatchSet::from_ids([]);
        let d = m.apply(&ids(&[10]), &[], |_| false);
        assert!(d.add.is_empty() && d.remove.is_empty());
        let d = m.apply(&[], &ids(&[10]), |_| true);
        assert_eq!(d.add, ids(&[10]));
        assert!(m.contains(ExchangeId(10)));
    }

    #[test]
    fn match_all_reports_new_ids() {
        let mut m = MatchSet::All;
        let mut calls = 0;
        let d = m.apply(&ids(&[5, 6]), &ids(&[1]), |_| {
            calls += 1;
            false
        });
        assert_eq!(calls, 0, "match-all never evaluates the filter");
        assert_eq!(d.add, ids(&[5, 6]));
        assert!(d.remove.is_empty());
        assert_eq!(m.sorted_ids(), None);
        assert!(m.contains(ExchangeId(999)));
    }

    #[test]
    fn removal_and_clear() {
        let mut m = MatchSet::from_ids(ids(&[1, 2, 3]));
        m.remove(&ids(&[2]));
        assert_eq!(m.sorted_ids(), Some(ids(&[1, 3])));
        // A removed id that comes back (re-imported with the same id can't happen, but an
        // update racing the removal) is re-added cleanly.
        let d = m.apply(&[], &ids(&[2]), |_| true);
        assert_eq!(d.add, ids(&[2]));
        m.clear();
        assert_eq!(m.sorted_ids(), Some(vec![]));
    }

    #[test]
    fn duplicate_ids_in_batch() {
        let mut m = MatchSet::from_ids([]);
        let d = m.apply(&ids(&[1]), &ids(&[1]), |_| true);
        assert_eq!(d.add, ids(&[1]));
    }
}
