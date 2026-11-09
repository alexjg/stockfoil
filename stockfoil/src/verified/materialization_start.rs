//! Select captured source inputs and decide whether old participation requires reset.
use super::{
    api_error::Error,
    change_plan,
    checkpoint::{Checkpoint, Follow},
    clock::NodeIdx,
    commit_hash::CommitHash,
    history::History,
    materialization_driver::Materialization,
    materialization_source::Source,
    view::{MaterializationCursor, ViewPoint},
    write_permissions::WritePermissions,
};
#[cfg(verus_keep_ghost)]
use super::{clock::below, materialization_source, permissions::coordinates, tree::Clock};
#[cfg(verus_keep_ghost)]
use crate::capture_refinement::frozen;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::{
    prelude::*,
    std_specs::{cmp::OrdSpec, iter::IteratorSpec},
};

verus! {

/// Before is deliberately not intersected with delivery: arbitrary stored cursors are accepted.
pub(crate) open spec fn visibility_differs(history: &History, cursor: &MaterializationCursor, target: &ViewPoint) -> bool {
    history.authorized_from(cursor.target_view().heads_view(), &cursor.target_view().permissions_view())
        != history.authorized_from(target.heads_view(), &target.permissions_view()).intersect(history.causal_from(cursor.delivery_view()))
}

proof fn clock_members_equal(history: &History, a: &Clock, b: &Clock, x: Set<CommitHash>, y: Set<CommitHash>)
    requires history.topology.valid(), frozen(a, history.read_state(), x), frozen(b, history.read_state(), y),
        coordinates(&history.topology, a), coordinates(&history.topology, b),
    ensures (a.contents() == b.contents()) == (x == y),
{
    history.representation(); history.topology.traversal_facts(); history.topology.permission_facts();
    let t = &history.topology;
    if x == y {
        assert forall|c: u32| a.contents().contains_key(c) implies b.contents().contains_key(c) && a.contents()[c]@ <= b.contents()[c]@ by {
            let n = a.contents()[c];
            let h = t.nodes_view()[n@ as int].hash;
            assert(t.hashes().contains_key(h) && t.hashes()[h]@ == n@);
            assert(history.known(h));
            assert(x.contains(h));
            assert(y.contains(h));
        }
        assert forall|c: u32| b.contents().contains_key(c) implies a.contents().contains_key(c) && b.contents()[c]@ <= a.contents()[c]@ by {
            let n = b.contents()[c];
            let h = t.nodes_view()[n@ as int].hash;
            assert(t.hashes().contains_key(h) && t.hashes()[h]@ == n@);
            assert(history.known(h));
            assert(y.contains(h));
            assert(x.contains(h));
        }
        assert forall|c: u32| a.contents().contains_key(c) implies a.contents()[c] == b.contents()[c] by {
            NodeIdx::same_index(a.contents()[c], b.contents()[c]);
        }
        assert(a.contents() =~= b.contents());
    }
    if a.contents() == b.contents() { assert(x =~= y); }
}

pub(crate) fn visibility_changed(history: &History, cursor: &MaterializationCursor, target: &ViewPoint) -> (out: Result<bool, Error>)
    ensures match out {
        Err(e) => e is UnknownCommit && !history.known(e->UnknownCommit_0)
            && (cursor.delivery_view().contains(e->UnknownCommit_0) || cursor.target_view().heads_view().contains(e->UnknownCommit_0)
                || target.heads_view().contains(e->UnknownCommit_0)),
        Ok(changed) => changed == visibility_differs(history, cursor, target)
            && change_plan::known(&history.topology, cursor.delivery_view())
            && change_plan::known(&history.topology, cursor.target_view().heads_view()) && change_plan::known(&history.topology, target.heads_view()),
    },
{
    proof { history.representation(); history.topology.established(); }
    let delivered = change_plan::delivery(&history.topology, cursor.delivery_heads())?;
    let before = change_plan::participation(&history.topology, cursor.view_point().heads(), cursor.view_point().write_permissions())?;
    let after = change_plan::participation(&history.topology, target.heads(), target.write_permissions())?;
    let bound = |chain: u32, _tip: NodeIdx| -> (limit: Option<NodeIdx>)
        ensures limit == delivered.lookup(chain),
    { delivered.position(chain) };
    let old_after = after.mask(bound);
    proof {
        let a = history.authorized_from(target.heads_view(), &target.permissions_view());
        let d = history.causal_from(cursor.delivery_view());
        assert forall|c: u32| old_after.contents().contains_key(c) implies
            old_after.contents()[c]@ < history.topology.labels().len()
                && history.topology.labels()[old_after.contents()[c]@ as int] == c by {
            assert(Clock::callback_bound(bound, c, after.contents()[c]) == delivered.lookup(c));
        }
        assert forall|h: CommitHash| history.known(h) implies a.intersect(d).contains(h)
            == below(old_after.lookup(history.read_state().labels[history.read_state().hashes[h]@ as int]), history.read_state().hashes[h]@ as usize) by {
            let c = history.read_state().labels[history.read_state().hashes[h]@ as int];
            if after.contents().contains_key(c) {
                assert(Clock::callback_bound(bound, c, after.contents()[c]) == delivered.lookup(c));
            }
        }
        assert(frozen(&old_after, history.read_state(), a.intersect(d)));
        clock_members_equal(history, &before, &old_after,
            history.authorized_from(cursor.target_view().heads_view(), &cursor.target_view().permissions_view()), a.intersect(d));
    }
    Ok(!before.same_map(&old_after))
}


fn sorted_hashes(values: &std::collections::BTreeSet<CommitHash>) -> (out: Vec<CommitHash>)
    ensures out@.to_set() == values@, out@.no_duplicates(), vstd::std_specs::btree::increasing_seq(out@),
{
    proof { super::stdlib::commit_order_model(); }
    let iterator = values.iter();
    let ghost hashes = iterator.remaining().unref();
    proof {
        broadcast use vstd::laws_cmp::lemma_ref_obeys_cmp_spec;
        assert(vstd::laws_cmp::obeys_cmp::<&CommitHash>());
        vstd::std_specs::btree::axiom_increasing_seq_meaning::<&CommitHash>(iterator.remaining());
        assert forall|i: int, j: int| 0 <= i < j < hashes.len() implies hashes[i].cmp_spec(&hashes[j]) is Less by {
            assert(<&CommitHash as vstd::std_specs::cmp::OrdSpec>::cmp_spec(&iterator.remaining()[i], &iterator.remaining()[j]) is Less);
        }
        assert(vstd::std_specs::btree::increasing_seq(hashes));
    }
    let mut out = Vec::new();
    for hash in iter: iterator
        invariant out@ == iter.seq().take(iter.index()).unref(), iter.seq().unref() == hashes,
            hashes.to_set() == values@, hashes.no_duplicates(), vstd::std_specs::btree::increasing_seq(hashes),
    {
        out.push(*hash);
        assert(iter.seq().take(iter.index() + 1).unref() =~= iter.seq().take(iter.index()).unref().push(*hash));
    }
    assert(out@ =~= hashes);
    out
}

impl History {
    pub(crate) proof fn head_set_facts(&self)
        ensures self.delivered_heads_set() == self.topology.head_hashes(), self.admitted_heads_set() == self.admitted_heads@,
    {}
    pub closed spec fn delivered_heads_set(&self) -> Set<CommitHash> { self.topology.head_hashes() }
    pub closed spec fn admitted_heads_set(&self) -> Set<CommitHash> { self.admitted_heads@ }
    pub open spec fn follow_heads_set(&self, follow: &Follow) -> Set<CommitHash> {
        match follow { Follow::Working => self.delivered_heads_set(), Follow::Signed => self.admitted_heads_set(), Follow::Pinned(heads) => heads@.to_set() }
    }
    pub open spec fn follow_target(&self, follow: &Follow, permissions: &WritePermissions, target: &ViewPoint) -> bool {
        target.permissions_view().same(permissions)
            && match follow {
                Follow::Pinned(heads) => target.heads_view() == heads@,
                _ => target.heads_view().to_set() == self.follow_heads_set(follow) && target.heads_view().no_duplicates()
                    && vstd::std_specs::btree::increasing_seq(target.heads_view()),
            }
    }
    pub(crate) fn captured_delivery_heads(&self) -> (out: Vec<CommitHash>)
        ensures out@.to_set() == self.delivered_heads_set(), out@.no_duplicates(), vstd::std_specs::btree::increasing_seq(out@),
    { sorted_hashes(self.topology.heads()) }

    /// Select stored working/signed heads or preserve the exact pin, and copy current policy.
    /// Signed-head selection is a storage statement, not a proof of authentication.
    pub(crate) fn follow_view(&self, follow: &Follow, permissions: &WritePermissions) -> (out: ViewPoint)
        ensures self.follow_target(follow, permissions, &out),
    {
        let heads = match follow {
            Follow::Working => self.captured_delivery_heads(),
            Follow::Signed => sorted_hashes(&self.admitted_heads),
            Follow::Pinned(heads) => heads.clone(),
        };
        ViewPoint::from_parts(heads, permissions.clone())
    }
}

pub(crate) open spec fn empty_base(base: &MaterializationCursor) -> bool {
    base.delivery_view().len() == 0 && base.target_view().heads_view().len() == 0
        && base.target_view().permissions_view().authors().is_empty() && base.target_view().permissions_view().fallback() is Denied
}

pub(crate) struct Inputs {
    pub base: MaterializationCursor,
    pub target: ViewPoint,
    pub delivery: Vec<CommitHash>,
    pub processed: u64,
    pub reset: bool,
}
impl Inputs {
    pub(crate) open spec fn matches(&self, history: &History, checkpoint: &Checkpoint, permissions: &WritePermissions, supports: bool) -> bool {
        match checkpoint.progress_view() {
            Some(progress) => self.base.same(&checkpoint.cursor_view()) && self.target.same(&progress.target)
                && self.delivery@ == progress.delivery_heads@ && self.processed == progress.applied && !self.reset,
            None => history.follow_target(&checkpoint.follow_view(), permissions, &self.target)
                && self.delivery@.to_set() == history.delivered_heads_set() && self.delivery@.no_duplicates()
                && vstd::std_specs::btree::increasing_seq(self.delivery@) && self.processed == 0
                && self.reset == (!supports && visibility_differs(history, &checkpoint.cursor_view(), &self.target))
                && (if self.reset { empty_base(&self.base) } else { self.base.same(&checkpoint.cursor_view()) }),
        }
    }
    pub(crate) open spec fn construction_error(&self, history: &History, error: Error) -> bool {
        let valid = materialization_source::valid_request(history, &self.base, &self.target, self.delivery@);
        ((error is UnknownCommit || error is InvalidItem) && !valid)
            || (error is Malformed && valid && self.processed > materialization_source::requested(history, &self.base, &self.target, self.delivery@).len())
    }
    pub(crate) open spec fn started(&self, history: &History, source: &Source) -> bool {
        materialization_source::valid_request(history, &self.base, &self.target, self.delivery@)
            && source.ready() && source.history_id() == history.history_id()
            && source.base() == self.base && source.target().same(&self.target) && source.delivery() == self.delivery@
            && source.processed() == self.processed && source.reset_pending() == self.reset && !source.finished()
            && source.total() == materialization_source::requested(history, &self.base, &self.target, self.delivery@).len()
            && source.processed() + source.selected().len() == source.total()
            && source.delivered_members() == history.causal_from(self.base.delivery_view())
            && source.before_members() == history.authorized_from(self.base.target_view().heads_view(), &self.base.target_view().permissions_view())
            && source.after_members() == history.authorized_from(self.target.heads_view(), &self.target.permissions_view())
            && exists|prefix: Seq<CommitHash>| prefix.len() == self.processed
                && change_plan::scheduled(history, materialization_source::requested(history, &self.base, &self.target, self.delivery@), prefix)
                && source.selected() == materialization_source::requested(history, &self.base, &self.target, self.delivery@).difference(prefix.to_set())
    }
}

pub(crate) open spec fn input_error(history: &History, checkpoint: &Checkpoint, supports: bool, error: Error) -> bool {
    checkpoint.progress_view() is None && !supports && error is UnknownCommit && !history.known(error->UnknownCommit_0)
        && (checkpoint.cursor_view().delivery_view().contains(error->UnknownCommit_0)
            || checkpoint.cursor_view().target_view().heads_view().contains(error->UnknownCommit_0)
            || history.follow_heads_set(&checkpoint.follow_view()).contains(error->UnknownCommit_0))
}

impl Checkpoint {
    pub(crate) fn source_inputs(&self, history: &History, permissions: &WritePermissions, supports: bool) -> (out: Result<Inputs, Error>)
        ensures match out {
            Ok(input) => input.matches(history, self, permissions, supports),
            Err(error) => input_error(history, self, supports, error),
        },
    {
        proof { self.representation(); }
        let (mut base, target, delivery, processed, reset) = match &self.in_progress {
            Some(progress) => (self.cursor.clone(), progress.target.clone(), progress.delivery_heads.clone(), progress.applied, false),
            None => {
                let target = history.follow_view(self.follow(), permissions);
                let reset = !supports && visibility_changed(history, &self.cursor, &target)?;
                (self.cursor.clone(), target, history.captured_delivery_heads(), 0, reset)
            },
        };
        if reset { base = MaterializationCursor::new(); }
        Ok(Inputs { base, target, delivery, processed, reset })
    }
}

impl Materialization {
    pub(crate) open spec fn same_outside_source(&self, before: &Self) -> bool {
        self.checkpoint == before.checkpoint && self.protocol == before.protocol && self.owner == before.owner
            && self.dirty == before.dirty && self.poisoned == before.poisoned && self.last == before.last
            && self.supports_visibility_changes == before.supports_visibility_changes
    }
    pub(crate) fn start_history(&mut self, history: &History, permissions: &WritePermissions) -> (out: Result<(), Error>)
        ensures final(self).same_outside_source(old(self)), match out {
            Ok(()) => final(self).source is Some && exists|input: Inputs|
                input.matches(history, &old(self).checkpoint, permissions, old(self).supports_visibility_changes)
                    && input.started(history, &final(self).source.unwrap()),
            Err(error) => *final(self) == *old(self) && (input_error(history, &old(self).checkpoint, old(self).supports_visibility_changes, error)
                || exists|input: Inputs| input.matches(history, &old(self).checkpoint, permissions, old(self).supports_visibility_changes)
                    && input.construction_error(history, error)),
        },
    {
        let input = self.checkpoint.source_inputs(history, permissions, self.supports_visibility_changes)?;
        let ghost chosen = input;
        let source = match Source::new(history, input.base, input.target, input.delivery, input.processed, input.reset) {
            Ok(source) => source,
            Err(error) => { assert(chosen.construction_error(history, error)); return Err(error); },
        };
        assert(chosen.started(history, &source));
        self.source = Some(source);
        assert(chosen.matches(history, &self.checkpoint, permissions, self.supports_visibility_changes));
        assert(chosen.started(history, &self.source.unwrap()));
        assert(exists|input: Inputs| input.matches(history, &self.checkpoint, permissions, self.supports_visibility_changes)
            && input.started(history, &self.source.unwrap()));
        Ok(())
    }
}

} // verus!
