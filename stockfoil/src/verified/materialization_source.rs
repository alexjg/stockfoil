//! Captured materialization progress, exact replay skipping and checkpoint preparation.
use super::{
    api_error::Error,
    change_plan::{self, Batch, Change, Changes},
    checkpoint::Checkpoint,
    commit_hash::CommitHash,
    history::History,
    view::{MaterializationCursor, ViewPoint},
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

struct State {
    changes: Changes,
    base: MaterializationCursor,
    processed: u64,
    reset: bool,
    done: Option<MaterializationCursor>,
    #[cfg(verus_keep_ghost)] total: Ghost<nat>,
}
impl State {
    spec fn valid(&self) -> bool {
        &&& self.total@ <= usize::MAX
        &&& self.processed + self.changes.selected().len() == self.total@
        &&& match self.done {
            None => true,
            Some(cursor) => self.changes.selected().is_empty()
                && cursor.delivery_view() == self.changes.delivery_view()
                && cursor.target_view().same(&self.changes.target_view()),
        }
    }
}

/// Taking the state before advancement leaves a poisoned source if emission unwinds.
pub(crate) struct Source { state: Option<State> }
pub(crate) struct SourceBatch { pub records: Vec<Change>, pub reset: bool }

pub(crate) open spec fn requested(history: &History, base: &MaterializationCursor, target: &ViewPoint,
    delivery: Seq<CommitHash>) -> Set<CommitHash> {
    change_plan::records(history.causal_from(delivery), history.causal_from(base.delivery_view()),
        history.authorized_from(base.target_view().heads_view(), &base.target_view().permissions_view()),
        history.authorized_from(target.heads_view(), &target.permissions_view()))
}

pub(crate) open spec fn valid_request(history: &History, base: &MaterializationCursor, target: &ViewPoint, delivery: Seq<CommitHash>) -> bool {
    change_plan::known(&history.topology, base.delivery_view()) && change_plan::known(&history.topology, base.target_view().heads_view())
        && change_plan::known(&history.topology, target.heads_view()) && change_plan::known(&history.topology, delivery)
        && base.delivery_view().to_set().union(base.target_view().heads_view().to_set()).union(target.heads_view().to_set()).subset_of(history.causal_from(delivery))
}

proof fn known_member(history: &History, heads: Seq<CommitHash>, hash: CommitHash)
    requires change_plan::known(&history.topology, heads), heads.contains(hash),
    ensures history.known(hash),
{
    history.representation();
    let i = choose|i: int| 0 <= i < heads.len() && heads[i] == hash;
    assert(history.topology.hashes().contains_key(heads[i]));
}

pub(crate) proof fn same_request(history: &History, a: &MaterializationCursor, b: &MaterializationCursor, x: &ViewPoint, y: &ViewPoint, delivery: Seq<CommitHash>)
    requires a.same(b), x.same(y),
    ensures valid_request(history, a, x, delivery) == valid_request(history, b, y, delivery),
        requested(history, a, x, delivery) == requested(history, b, y, delivery),
{
    a.target_view().same_authorization(&b.target_view(), history);
    x.same_authorization(y, history);
}

/// Two canonical traversals have the same prefix regardless of chunk boundaries.
pub(crate) proof fn schedule_prefix(history: &History, initial: Set<CommitHash>, a: Seq<CommitHash>, b: Seq<CommitHash>, n: nat)
    requires change_plan::scheduled(history, initial, a), change_plan::scheduled(history, initial, b), n <= a.len(), n <= b.len(),
    ensures a.take(n as int) == b.take(n as int),
    decreases n,
{
    if n > 0 {
        schedule_prefix(history, initial, a, b, (n - 1) as nat);
        let i = n as int - 1;
        let remaining = initial.difference(a.take(i).to_set());
        assert(change_plan::eligible(history, remaining, a[i]));
        assert(change_plan::eligible(history, remaining, b[i]));
        assert(change_plan::hash_le(a[i], b[i]) && change_plan::hash_le(b[i], a[i]));
        if a[i] != b[i] { super::canonical_order::hash_asymmetric(a[i], b[i]); }
        assert(a.take(n as int) =~= a.take(i).push(a[i]));
        assert(b.take(n as int) =~= b.take(i).push(b[i]));
    }
}

proof fn concat_set(a: Seq<CommitHash>, b: Seq<CommitHash>)
    ensures (a + b).to_set() == a.to_set().union(b.to_set()),
{
    assert forall|h: CommitHash| (a + b).to_set().contains(h) == a.to_set().union(b.to_set()).contains(h) by {
        if (a + b).to_set().contains(h) {
            let i = choose|i: int| 0 <= i < (a + b).len() && (a + b)[i] == h;
            if i < a.len() { assert(a[i] == h); } else { assert(b[i - a.len()] == h); }
        }
        if a.to_set().contains(h) {
            let i = choose|i: int| 0 <= i < a.len() && a[i] == h;
            assert((a + b)[i] == h);
        }
        if b.to_set().contains(h) {
            let i = choose|i: int| 0 <= i < b.len() && b[i] == h;
            assert((a + b)[a.len() + i] == h);
        }
    }
    assert((a + b).to_set() =~= a.to_set().union(b.to_set()));
}

pub(crate) proof fn schedule_append(history: &History, initial: Set<CommitHash>, a: Seq<CommitHash>, b: Seq<CommitHash>)
    requires change_plan::scheduled(history, initial, a),
        change_plan::scheduled(history, initial.difference(a.to_set()), b),
    ensures change_plan::scheduled(history, initial, a + b),
{
    assert forall|i: int| #![trigger (a + b)[i]] 0 <= i < (a + b).len() implies {
        let remaining = initial.difference((a + b).take(i).to_set());
        change_plan::eligible(history, remaining, (a + b)[i])
            && forall|h: CommitHash| change_plan::eligible(history, remaining, h) ==> change_plan::hash_le((a + b)[i], h)
    } by {
        if i < a.len() { assert((a + b).take(i) =~= a.take(i)); }
        else {
            let j = i - a.len();
            assert((a + b).take(i) =~= a + b.take(j));
            concat_set(a, b.take(j));
            assert((a + b.take(j)).to_set() =~= a.to_set().union(b.take(j).to_set()));
            assert(initial.difference((a + b).take(i).to_set()) =~= initial.difference(a.to_set()).difference(b.take(j).to_set()));
            assert(change_plan::eligible(history, initial.difference(a.to_set()).difference(b.take(j).to_set()), b[j]));
        }
    }
}

impl Source {
    #[verifier::type_invariant]
    spec fn invariant(self) -> bool { self.state is Some ==> self.state.unwrap().valid() }
    pub(crate) closed spec fn ready(&self) -> bool { self.state is Some }
    pub(crate) closed spec fn selected(&self) -> Set<CommitHash> { self.state.unwrap().changes.selected() }
    pub(crate) closed spec fn history_id(&self) -> u64 { self.state.unwrap().changes.history_id() }
    pub(crate) closed spec fn processed(&self) -> u64 { self.state.unwrap().processed }
    pub(crate) closed spec fn total(&self) -> nat { self.state.unwrap().total@ }
    pub(crate) closed spec fn base(&self) -> MaterializationCursor { self.state.unwrap().base }
    pub(crate) closed spec fn target(&self) -> ViewPoint { self.state.unwrap().changes.target_view() }
    pub(crate) closed spec fn delivery(&self) -> Seq<CommitHash> { self.state.unwrap().changes.delivery_view() }
    pub(crate) closed spec fn reset_pending(&self) -> bool { self.state.unwrap().reset }
    pub(crate) closed spec fn finished(&self) -> bool { self.ready() && self.state.unwrap().done is Some }
    pub(crate) closed spec fn delivered_members(&self) -> Set<CommitHash> { self.state.unwrap().changes.delivered_members() }
    pub(crate) closed spec fn before_members(&self) -> Set<CommitHash> { self.state.unwrap().changes.before_members() }
    pub(crate) closed spec fn after_members(&self) -> Set<CommitHash> { self.state.unwrap().changes.after_members() }

    pub(crate) fn new(history: &History, base: MaterializationCursor, target: ViewPoint,
        delivery: Vec<CommitHash>, processed: u64, reset: bool) -> (result: Result<Self, Error>)
        ensures match result {
            Err(e) => ((e is UnknownCommit || e is InvalidItem) && !valid_request(history, &base, &target, delivery@))
                || (e is Malformed && valid_request(history, &base, &target, delivery@) && processed > requested(history, &base, &target, delivery@).len()),
            Ok(source) => valid_request(history, &base, &target, delivery@) && source.ready() && source.history_id() == history.history_id()
                && source.base() == base && source.target().same(&target) && source.delivery() == delivery@
                && source.processed() == processed && source.total() == requested(history, &base, &target, delivery@).len()
                && source.reset_pending() == reset && !source.finished()
                && source.delivered_members() == history.causal_from(base.delivery_view())
                && source.before_members() == history.authorized_from(base.target_view().heads_view(), &base.target_view().permissions_view())
                && source.after_members() == history.authorized_from(target.heads_view(), &target.permissions_view())
                && source.processed() + source.selected().len() == source.total()
                && exists|prefix: Seq<CommitHash>| prefix.len() == processed
                    && change_plan::scheduled(history, requested(history, &base, &target, delivery@), prefix)
                    && source.selected() == requested(history, &base, &target, delivery@).difference(prefix.to_set()),
        },
    {
        let mut changes = match change_plan::plan_changes_at(history, &base, &target, &delivery) {
            Ok(c) => c,
            Err(Error::UnknownCommit(h)) => {
                proof {
                    if valid_request(history, &base, &target, delivery@) {
                        if base.delivery_view().contains(h) { known_member(history, base.delivery_view(), h); }
                        else if base.target_view().heads_view().contains(h) { known_member(history, base.target_view().heads_view(), h); }
                        else if target.heads_view().contains(h) { known_member(history, target.heads_view(), h); }
                        else { known_member(history, delivery@, h); }
                    }
                }
                return Err(Error::UnknownCommit(h));
            },
            Err(e) => return Err(e),
        };
        let total = changes.remaining();
        if processed > total as u64 { return Err(Error::Malformed("checkpoint record count exceeds captured traversal")); }
        let ghost initial = changes;
        let ghost mut prefix = Seq::<CommitHash>::empty();
        let mut skipped = Vec::new();
        let mut left = processed as usize;
        while left > 0
            invariant left <= processed <= total, total == initial.selected().len(),
                changes.history_id() == initial.history_id() == history.history_id(),
                changes.delivery_view() == initial.delivery_view(), changes.target_view() == initial.target_view(),
                changes.delivered_members() == initial.delivered_members(), changes.before_members() == initial.before_members(), changes.after_members() == initial.after_members(),
                prefix.len() + left == processed,
                changes.selected().len() + processed - left == total,
                changes.selected() == initial.selected().difference(prefix.to_set()),
                change_plan::scheduled(history, initial.selected(), prefix),
            decreases left,
        {
            skipped.clear();
            let count = if left < 1024 { left } else { 1024 };
            let ghost previous = prefix;
            let _status = change_plan::emit_next_changes(history, &mut changes, &mut skipped, count).unwrap();
            assert(skipped@.skip(0) =~= skipped@);
            assert(skipped.len() == count);
            proof {
                schedule_append(history, initial.selected(), previous, change_plan::hashes(skipped@));
                concat_set(previous, change_plan::hashes(skipped@));
                prefix = previous + change_plan::hashes(skipped@);
                assert(prefix.to_set() =~= previous.to_set().union(change_plan::hashes(skipped@).to_set()));
                assert(changes.selected() =~= initial.selected().difference(prefix.to_set()));
            }
            left -= count;
        }
        let source = Self { state: Some(State { changes, base, processed, reset, done: None,
            #[cfg(verus_keep_ghost)] total: Ghost(total as nat) }) };
        assert(prefix.len() == processed && change_plan::scheduled(history, initial.selected(), prefix));
        assert(source.selected() == initial.selected().difference(prefix.to_set()));
        Ok(source)
    }

    pub(crate) fn is_done(&self) -> (out: bool) ensures out == self.finished(),
    { match &self.state { Some(s) => s.done.is_some(), None => false } }

    /// One source batch, reserving one budget slot for a pending Reset event.
    pub(crate) fn advance(&mut self, history: &History, budget: usize) -> (result: Result<SourceBatch, Error>)
        ensures match result {
            Err(e) => *final(self) == *old(self) && (if !old(self).ready() { e is Poisoned } else { e is WrongHistory && old(self).history_id() != history.history_id() }),
            Ok(batch) => final(self).ready() && old(self).ready() && old(self).history_id() == history.history_id()
                && final(self).history_id() == old(self).history_id() && final(self).total() == old(self).total()
                && final(self).base() == old(self).base() && final(self).target() == old(self).target() && final(self).delivery() == old(self).delivery()
                && final(self).delivered_members() == old(self).delivered_members() && final(self).before_members() == old(self).before_members() && final(self).after_members() == old(self).after_members()
                && batch.reset == old(self).reset_pending() && !final(self).reset_pending()
                && batch.records.len() == vstd::math::min(old(self).selected().len() as int, if batch.reset && budget > 0 { budget as int - 1 } else { budget as int })
                && final(self).processed() == old(self).processed() + batch.records.len()
                && final(self).processed() + final(self).selected().len() == final(self).total()
                && final(self).selected() == old(self).selected().difference(change_plan::hashes(batch.records@).to_set())
                && change_plan::scheduled(history, old(self).selected(), change_plan::hashes(batch.records@))
                && change_plan::hashes(batch.records@).no_duplicates()
                && (forall|i: int| 0 <= i < batch.records.len() ==> change_plan::mapped(history, batch.records[i], old(self).delivered_members(), old(self).before_members(), old(self).after_members()))
                && final(self).finished() == final(self).selected().is_empty(),
        },
    {
        proof { use_type_invariant(&*self); }
        let state = match &self.state { Some(s) => s, None => return Err(Error::Poisoned) };
        proof { state.changes.metadata(); }
        match state.changes.plan.check_history(history) { Ok(()) => {}, Err(e) => return Err(e) }
        let mut state = self.state.take().unwrap();
        let reset = state.reset;
        state.reset = false;
        let slots = if reset { budget.saturating_sub(1) } else { budget };
        let mut records = Vec::new();
        let status = change_plan::emit_next_changes(history, &mut state.changes, &mut records, slots).unwrap();
        assert(records@.skip(0) =~= records@);
        state.processed += records.len() as u64;
        match status { Batch::Complete(cursor) => state.done = Some(cursor), Batch::More => {} }
        self.state = Some(state);
        Ok(SourceBatch { records, reset })
    }

    pub(crate) fn checkpoint(&self, published: &Checkpoint) -> (result: Result<Checkpoint, Error>)
        ensures match result {
            Err(e) => e is Poisoned && !self.ready(),
            Ok(next) => self.ready() && next.follow_view().same(&published.follow_view()) && next.rejected_view() == published.rejected_view()
                && (next.progress_view() is None) == self.finished()
                && if self.finished() {
                    self.processed() == self.total() && next.cursor_view().delivery_view() == self.delivery() && next.cursor_view().target_view().same(&self.target())
                } else {
                    next.cursor_view().same(&self.base()) && next.progress_view() is Some
                        && next.progress_view().unwrap().target.same(&self.target()) && next.progress_view().unwrap().delivery_heads@ == self.delivery()
                        && next.progress_view().unwrap().applied == self.processed() && self.processed() + self.selected().len() == self.total()
                },
        },
    {
        proof { use_type_invariant(self); }
        let s = match &self.state { Some(s) => s, None => return Err(Error::Poisoned) };
        proof { s.changes.metadata(); }
        Ok(published.prepare_next(&s.base, &s.changes.target, &s.changes.delivery_heads, s.processed, &s.done))
    }

    pub(crate) fn restart_checkpoint(&self, next: &mut Checkpoint) -> (result: Result<(), Error>)
        ensures match result {
            Err(e) => e is Poisoned && !self.ready() && *final(next) == *old(next),
            Ok(()) => self.ready() && final(next).follow_view() == old(next).follow_view() && final(next).rejected_view() == old(next).rejected_view()
                && final(next).cursor_view().delivery_view().len() == 0 && final(next).cursor_view().target_view().heads_view().len() == 0
                && final(next).progress_view() is Some && final(next).progress_view().unwrap().applied == 0
                && final(next).progress_view().unwrap().target.same(&self.target()) && final(next).progress_view().unwrap().delivery_heads@ == self.delivery(),
        },
    {
        let s = match &self.state { Some(s) => s, None => return Err(Error::Poisoned) };
        proof { s.changes.metadata(); }
        next.restart(&s.changes.target, &s.changes.delivery_heads);
        Ok(())
    }
}

} // verus!

#[cfg(test)]
mod tests {
    use super::super::author::Author;
    use super::*;

    fn hash(n: u64) -> CommitHash {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&n.to_be_bytes());
        CommitHash(bytes)
    }

    #[test]
    fn resume_crosses_skip_chunk_and_excludes_later_growth() {
        let mut history = History::new();
        for i in 0..1030 {
            let parents = if i == 0 { vec![] } else { vec![hash(i - 1)] };
            history
                .topology
                .insert(hash(i), Author([1; 32]), &parents, i + 50, 0)
                .unwrap();
        }
        let published = Checkpoint::default();
        let mut source = Source::new(
            &history,
            MaterializationCursor::new(),
            ViewPoint::default(),
            vec![hash(1029)],
            1025,
            false,
        )
        .unwrap();
        let before = source.checkpoint(&published).unwrap().to_bytes();
        let wrong = History::new();
        assert!(matches!(
            source.advance(&wrong, 0),
            Err(Error::WrongHistory)
        ));
        assert_eq!(source.checkpoint(&published).unwrap().to_bytes(), before);
        history
            .topology
            .insert(hash(1030), Author([1; 32]), &[hash(1029)], 1080, 0)
            .unwrap();
        let first = source.advance(&history, 2).unwrap();
        assert!(!first.reset);
        assert_eq!(
            first
                .records
                .iter()
                .map(|r| (r.hash, r.payload))
                .collect::<Vec<_>>(),
            vec![(hash(1025), 1075), (hash(1026), 1076)]
        );
        let partial = source.checkpoint(&published).unwrap();
        assert_eq!(partial.in_progress.as_ref().unwrap().applied, 1027);
        assert_eq!(
            partial.in_progress.as_ref().unwrap().delivery_heads,
            vec![hash(1029)]
        );
        let last = source.advance(&history, usize::MAX).unwrap();
        assert_eq!(
            last.records.iter().map(|r| r.hash).collect::<Vec<_>>(),
            vec![hash(1027), hash(1028), hash(1029)]
        );
        assert!(source.is_done());
        let completed = source.checkpoint(&published).unwrap();
        assert!(!completed.is_partial());
        assert_eq!(completed.cursor.delivery_heads(), &[hash(1029)]);
        assert!(source.advance(&history, 0).unwrap().records.is_empty());
        assert!(matches!(
            source.advance(&wrong, usize::MAX),
            Err(Error::WrongHistory)
        ));
        assert_eq!(source.checkpoint(&published).unwrap(), completed);
        assert!(matches!(
            Source::new(
                &history,
                MaterializationCursor::new(),
                ViewPoint::default(),
                vec![hash(1029)],
                1031,
                false
            ),
            Err(Error::Malformed(_))
        ));
    }

    #[test]
    fn reset_reserves_a_slot_and_taken_state_fails_closed() {
        let mut history = History::new();
        history
            .topology
            .insert(hash(1), Author([1; 32]), &[], 42, 0)
            .unwrap();
        let mut source = Source::new(
            &history,
            MaterializationCursor::new(),
            ViewPoint::default(),
            vec![hash(1)],
            0,
            true,
        )
        .unwrap();
        let reset = source.advance(&history, 1).unwrap();
        assert!(reset.reset && reset.records.is_empty());
        assert_eq!(
            source
                .checkpoint(&Checkpoint::default())
                .unwrap()
                .in_progress
                .unwrap()
                .applied,
            0
        );
        let data = source.advance(&history, 1).unwrap();
        assert!(!data.reset && data.records.len() == 1 && source.is_done());
        // This is the representation left behind if owned advancement unwinds.
        let _taken = source.state.take();
        assert!(!source.is_done());
        assert!(matches!(source.advance(&history, 1), Err(Error::Poisoned)));
        assert!(matches!(
            source.checkpoint(&Checkpoint::default()),
            Err(Error::Poisoned)
        ));
        let mut next = Checkpoint::new(super::super::checkpoint::Follow::Signed);
        let saved = next.to_bytes();
        assert!(matches!(
            source.restart_checkpoint(&mut next),
            Err(Error::Poisoned)
        ));
        assert_eq!(next.to_bytes(), saved);
    }
}
