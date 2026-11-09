//! Validate application reports before consuming the pending batch token.
use super::{
    api_error::Error,
    batch_protocol::{Batch, Protocol},
    checkpoint::Checkpoint,
    commit_hash::CommitHash,
    history::History,
    materialization_events::Counts,
};
use std::collections::{BTreeSet, HashSet};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// Feedback returned after applying a batch of [`Event`](super::batch_protocol::Event)s.
#[cfg_attr(not(verus_keep_ghost), derive(Debug, Clone, PartialEq, Eq))]
pub struct Applied {
    /// Commits newly delivered by the pending source batch whose payloads are
    /// semantically invalid. They may also be reported during its immediate
    /// follow-up rounds.
    pub rejected: Vec<Rejection>,
    /// Previously delivered commits whose payloads the application needs
    /// delivered in an immediate follow-up [`Event::Payload`](super::batch_protocol::Event::Payload) batch (for
    /// example, when retracting a winning write uncovers an older overwritten
    /// commit whose payload was not kept in memory).
    pub wanted: Vec<CommitHash>,
}
/// Permanent per-materialization rejection of a commit with an invalid
/// application payload.
///
/// Rejecting a commit excludes its effects from this materialization (and is
/// recorded in its [`Checkpoint`]) without removing the commit from [`History`].
/// If `cascade` is `true`, any causal descendant of `hash` is also treated as
/// rejected. `reason` is diagnostic only and is not persisted in checkpoints.
#[cfg_attr(not(verus_keep_ghost), derive(Debug, Clone, PartialEq, Eq))]
pub struct Rejection {
    pub hash: CommitHash,
    pub reason: String,
    pub cascade: bool,
}
pub(crate) struct Pending {
    #[cfg(verus_keep_ghost)] pub(crate) origin: Ghost<Counts>,
    #[cfg(verus_keep_ghost)] pub(crate) original_active: Ghost<Seq<CommitHash>>,
    pub(crate) next: Checkpoint,
    pub(crate) delivered: HashSet<CommitHash>,
    pub(crate) active: Vec<CommitHash>,
    pub(crate) corrected: HashSet<CommitHash>,
    pub(crate) restart: bool,
    pub(crate) counts: Counts,
}

impl Pending {
    pub(crate) fn new(next: Checkpoint, summary: super::materialization_events::Summary) -> (out: Self)
        ensures out.origin@ == summary.counts, out.original_active@ == summary.active@,
            out.next == next, out.delivered@ == summary.delivered@, out.active@ == summary.active@,
            out.counts == summary.counts, out.corrected@ == Set::<CommitHash>::empty(), !out.restart,
    {
        Self { #[cfg(verus_keep_ghost)] origin: Ghost(summary.counts),
            #[cfg(verus_keep_ghost)] original_active: Ghost(summary.active@),
            next, delivered: summary.delivered, active: summary.active, counts: summary.counts,
            corrected: HashSet::new(), restart: false }
    }
}

impl Default for Applied {
    fn default() -> (out: Self) ensures out.rejected.len() == 0, out.wanted.len() == 0,
    { Self { rejected: Vec::new(), wanted: Vec::new() } }
}
impl Applied {
    /// Request that `hash`'s payload be delivered in an immediate follow-up
    /// [`Event::Payload`](super::batch_protocol::Event::Payload) batch.
    pub fn want(mut self, hash: CommitHash) -> (out: Self)
        ensures out.rejected@ == self.rejected@, out.wanted@ == self.wanted@.push(hash),
    { self.wanted.push(hash); self }
}

pub(crate) struct Validated {
    pub rejected: Vec<Rejection>,
    pub wanted: BTreeSet<CommitHash>,
}

pub(crate) open spec fn rejection_hashes(rejected: Seq<Rejection>) -> Seq<CommitHash> {
    rejected.map(|i: int, r: Rejection| r.hash)
}
pub(crate) open spec fn distinct_delivered(delivered: Set<CommitHash>, rejections: Seq<Rejection>) -> bool {
    rejection_hashes(rejections).no_duplicates() && rejection_hashes(rejections).to_set().subset_of(delivered)
}
pub(crate) open spec fn valid_rejections(delivered: Set<CommitHash>, applied: &Applied) -> bool {
    distinct_delivered(delivered, applied.rejected@)
}
pub(crate) open spec fn valid_wanted(history: &History, applied: &Applied) -> bool {
    forall|h: CommitHash| applied.wanted@.contains(h) ==> history.known(h)
}
impl Validated {
    pub(crate) open spec fn matches(&self, applied: &Applied) -> bool {
        self.rejected@ == applied.rejected@ && self.wanted@ == applied.wanted@.to_set()
    }
}

fn check_rejections(delivered: &HashSet<CommitHash>, rejections: &[Rejection]) -> (out: Result<(), Error>)
    ensures match out {
        Ok(()) => distinct_delivered(delivered@, rejections@),
        Err(e) => e is Protocol && !distinct_delivered(delivered@, rejections@),
    },
{
    proof { super::stdlib::commit_key_model(); }
    let mut seen = HashSet::new();
    let mut i = 0usize;
    while i < rejections.len()
        invariant i <= rejections.len(),
            seen@ == rejection_hashes(rejections@.take(i as int)).to_set(),
            rejection_hashes(rejections@.take(i as int)).no_duplicates(),
            seen@.subset_of(delivered@),
            vstd::std_specs::hash::obeys_key_model::<CommitHash>(),
        decreases rejections.len() - i,
    {
        let hash = rejections[i].hash;
        let ghost prefix = rejection_hashes(rejections@.take(i as int));
        if !delivered.contains(&hash) {
            assert(rejection_hashes(rejections@)[i as int] == hash);
            return Err(Error::Protocol("rejections must name distinct delivered commits from this batch"));
        }
        if !seen.insert(hash) {
            proof {
                let j = choose|j: int| 0 <= j < i && prefix[j] == hash;
                assert(rejection_hashes(rejections@)[j] == hash);
                assert(rejection_hashes(rejections@)[i as int] == hash);
            }
            return Err(Error::Protocol("rejections must name distinct delivered commits from this batch"));
        }
        assert(rejection_hashes(rejections@.take(i + 1)) =~= prefix.push(hash));
        proof { prefix.lemma_push_to_set_commute(hash); }
        i += 1;
    }
    assert(rejections@.take(i as int) =~= rejections@);
    Ok(())
}

fn check_wanted(history: &History, hashes: Vec<CommitHash>) -> (out: Result<BTreeSet<CommitHash>, Error>)
    ensures match out {
        Ok(wanted) => wanted@ == hashes@.to_set() && forall|h: CommitHash| wanted@.contains(h) ==> history.known(h),
        Err(e) => match e { Error::UnknownCommit(h) => hashes@.contains(h) && !history.known(h), _ => false },
    },
{
    proof { super::stdlib::commit_order_model(); }
    let mut wanted = BTreeSet::new();
    let mut j = 0usize;
    while j < hashes.len()
        invariant j <= hashes.len(), wanted@ == hashes@.take(j as int).to_set(),
            vstd::laws_cmp::obeys_cmp::<CommitHash>(),
        decreases hashes.len() - j,
    {
        wanted.insert(hashes[j]);
        assert(hashes@.take(j + 1) =~= hashes@.take(j as int).push(hashes[j as int]));
        proof { hashes@.take(j as int).lemma_push_to_set_commute(hashes[j as int]); }
        j += 1;
    }
    assert(hashes@.take(j as int) =~= hashes@);
    proof { history.representation(); }
    for hash in iter: wanted.iter()
        invariant wanted@ == hashes@.to_set(),
            history.read_state() == history.topology.read_state(),
            iter.seq().unref().to_set() == wanted@,
            forall|i: int| 0 <= i < iter.index() ==> history.known(*iter.seq()[i]),
    {
        assert(iter.seq().unref()[iter.index()] == *hash);
        assert(wanted@.contains(*hash));
        if !history.topology.by_hash().contains_key(hash) { return Err(Error::UnknownCommit(*hash)); }
    }
    Ok(wanted)
}

pub(crate) fn validate(history: &History, delivered: &HashSet<CommitHash>, applied: Applied) -> (out: Result<Validated, Error>)
    ensures match out {
        Ok(report) => report.matches(&applied) && valid_rejections(delivered@, &applied) && valid_wanted(history, &applied),
        Err(error) => match error {
            Error::Protocol(_) => !valid_rejections(delivered@, &applied),
            Error::UnknownCommit(hash) => valid_rejections(delivered@, &applied) && applied.wanted@.contains(hash) && !history.known(hash),
            _ => false,
        },
    },
{
    check_rejections(delivered, &applied.rejected)?;
    let wanted = check_wanted(history, applied.wanted)?;
    Ok(Validated { rejected: applied.rejected, wanted })
}

pub(crate) open spec fn cascade_for(rejections: Seq<Rejection>, hash: CommitHash) -> bool {
    exists|i: int| 0 <= i < rejections.len() && rejections[i].hash == hash && rejections[i].cascade
}
pub(crate) open spec fn merged(base: Map<CommitHash, bool>, rejections: Seq<Rejection>) -> Map<CommitHash, bool> {
    Map::new(base.dom().union(rejection_hashes(rejections).to_set()),
        |h: CommitHash| (base.contains_key(h) && base[h]) || cascade_for(rejections, h))
}
proof fn merge_push(base: Map<CommitHash, bool>, rejections: Seq<Rejection>, next: Rejection)
    ensures merged(base, rejections.push(next)) == merged(base, rejections).insert(next.hash,
        next.cascade || (merged(base, rejections).contains_key(next.hash) && merged(base, rejections)[next.hash])),
{
    assert(rejection_hashes(rejections.push(next)) =~= rejection_hashes(rejections).push(next.hash));
    rejection_hashes(rejections).lemma_push_to_set_commute(next.hash);
    assert forall|h: CommitHash| cascade_for(rejections, h) implies rejection_hashes(rejections).contains(h) by {
        let i = choose|i: int| 0 <= i < rejections.len() && rejections[i].hash == h && rejections[i].cascade;
        assert(rejection_hashes(rejections)[i] == h);
    }
    assert forall|h: CommitHash| #![trigger cascade_for(rejections.push(next), h)] cascade_for(rejections.push(next), h) == (cascade_for(rejections, h) || h == next.hash && next.cascade) by {
        if cascade_for(rejections.push(next), h) {
            let i = choose|i: int| 0 <= i < rejections.push(next).len() && rejections.push(next)[i].hash == h && rejections.push(next)[i].cascade;
            if i < rejections.len() { assert(cascade_for(rejections, h)); }
        }
        if h == next.hash && next.cascade { assert(rejections.push(next)[rejections.len() as int] == next); }
        if cascade_for(rejections, h) {
            let i = choose|i: int| 0 <= i < rejections.len() && rejections[i].hash == h && rejections[i].cascade;
            assert(rejections.push(next)[i] == rejections[i]);
        }
    }
    assert(merged(base, rejections.push(next)) =~= merged(base, rejections).insert(next.hash,
        next.cascade || (merged(base, rejections).contains_key(next.hash) && merged(base, rejections)[next.hash])));
}

impl Validated {
    /// Record exactly the supplied rejections; count only previously unrecorded hashes.
    pub(crate) fn record_rejections(&self, checkpoint: &mut Checkpoint) -> (added: usize)
        ensures final(checkpoint).rejected_view() == merged(old(checkpoint).rejected_view(), self.rejected@),
            added == rejection_hashes(self.rejected@).to_set().difference(old(checkpoint).rejected_view().dom()).len(),
            added <= self.rejected.len(),
            final(checkpoint).follow_view() == old(checkpoint).follow_view(),
            final(checkpoint).cursor_view() == old(checkpoint).cursor_view(),
            final(checkpoint).progress_view() == old(checkpoint).progress_view(),
    {
        let ghost base = checkpoint.rejected_view();
        let mut added = 0usize;
        let mut i = 0usize;
        assert(merged(base, Seq::empty()) =~= base);
        assert(rejection_hashes(self.rejected@.take(0)) =~= Seq::<CommitHash>::empty());
        assert(rejection_hashes(self.rejected@.take(0)).to_set().difference(base.dom()) =~= Set::<CommitHash>::empty());
        while i < self.rejected.len()
            invariant i <= self.rejected.len(), added <= i,
                checkpoint.rejected_view() == merged(base, self.rejected@.take(i as int)),
                added == rejection_hashes(self.rejected@.take(i as int)).to_set().difference(base.dom()).len(),
                checkpoint.follow_view() == old(checkpoint).follow_view(),
                checkpoint.cursor_view() == old(checkpoint).cursor_view(),
                checkpoint.progress_view() == old(checkpoint).progress_view(),
            decreases self.rejected.len() - i,
        {
            let rejected = &self.rejected[i];
            let ghost prefix = self.rejected@.take(i as int);
            let ghost prior = checkpoint.rejected_view();
            let ghost new_hashes = rejection_hashes(prefix).to_set().difference(base.dom());
            let fresh = checkpoint.reject(rejected.hash, rejected.cascade);
            proof {
                merge_push(base, prefix, *rejected);
                assert(self.rejected@.take(i + 1) =~= prefix.push(*rejected));
                assert(rejection_hashes(prefix.push(*rejected)) =~= rejection_hashes(prefix).push(rejected.hash));
                rejection_hashes(prefix).lemma_push_to_set_commute(rejected.hash);
                if base.contains_key(rejected.hash) {
                    assert(rejection_hashes(prefix.push(*rejected)).to_set().difference(base.dom()) =~= new_hashes);
                } else {
                    assert(rejection_hashes(prefix.push(*rejected)).to_set().difference(base.dom()) =~= new_hashes.insert(rejected.hash));
                }
            }
            if fresh { added += 1; }
            i += 1;
        }
        assert(self.rejected@.take(i as int) =~= self.rejected@);
        added
    }
}

/// Check the receipt, then the entire report, before the only consuming operation.
pub(crate) fn accept_report(protocol: &mut Protocol<Pending>, history: &History, batch: &Batch, applied: Applied) -> (out: Result<(Pending, Validated), Error>)
    ensures final(protocol).planner_id() == old(protocol).planner_id(), final(protocol).sequence() == old(protocol).sequence(),
        match out {
            Err(error) => *final(protocol) == *old(protocol) && match error {
                Error::StaleBatch => !old(protocol).accepts(batch),
                Error::Protocol(_) => old(protocol).accepts(batch) && !valid_rejections(old(protocol).pending_value().unwrap().delivered@, &applied),
                Error::UnknownCommit(hash) => old(protocol).accepts(batch)
                    && valid_rejections(old(protocol).pending_value().unwrap().delivered@, &applied)
                    && applied.wanted@.contains(hash) && !history.known(hash),
                _ => false,
            },
            Ok((pending, report)) => old(protocol).accepts(batch) && old(protocol).pending_value() == Some(pending)
                && final(protocol).pending_value() is None && report.matches(&applied)
                && valid_rejections(pending.delivered@, &applied) && valid_wanted(history, &applied),
        },
{
    let pending = protocol.check(batch)?;
    let report = validate(history, &pending.delivered, applied)?;
    let pending = protocol.take(batch).unwrap();
    Ok((pending, report))
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_counts_new_hashes_and_never_weakens_cascades() {
        let [existing, fresh] = [1, 2].map(|n| CommitHash([n; 32]));
        let mut checkpoint = Checkpoint::default();
        checkpoint.reject(existing, true);
        let saved = checkpoint.clone();
        let report = Validated {
            rejected: vec![
                Rejection {
                    hash: existing,
                    reason: "already rejected".into(),
                    cascade: false,
                },
                Rejection {
                    hash: fresh,
                    reason: "direct".into(),
                    cascade: false,
                },
                Rejection {
                    hash: fresh,
                    reason: "upgrade".into(),
                    cascade: true,
                },
                Rejection {
                    hash: fresh,
                    reason: "repeat".into(),
                    cascade: false,
                },
            ],
            wanted: BTreeSet::new(),
        };
        assert_eq!(report.record_rejections(&mut checkpoint), 1);
        assert_eq!(
            checkpoint.rejected().collect::<Vec<_>>(),
            vec![(existing, true), (fresh, true)]
        );
        assert_eq!(report.record_rejections(&mut checkpoint), 0);
        assert_eq!(checkpoint.follow, saved.follow);
        assert_eq!(checkpoint.cursor, saved.cursor);
        assert_eq!(checkpoint.in_progress, saved.in_progress);
    }
}
