//! Public planning against the actual Ledger representation.
use super::{
    api_error::Error, batch_protocol::Batch, ledger::Ledger,
    materialization_driver::Materialization, materialization_events as events,
    materialization_feedback::Pending,
};
#[cfg(verus_keep_ghost)]
use super::{
    batch_protocol::EventView, commit_hash::CommitHash, history::History,
    write_permissions::WritePermissions,
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::{
    prelude::*,
    std_specs::cmp::{OrdSpec, PartialOrdSpec},
};

verus! {

pub(crate) proof fn tail_set(a: Seq<CommitHash>)
    requires a.len() > 0, a.no_duplicates(),
    ensures a.drop_first().to_set() == a.to_set().remove(a[0]),
{
    assert forall|h: CommitHash| a.drop_first().to_set().contains(h) == a.to_set().remove(a[0]).contains(h) by {
        if a.drop_first().contains(h) {
            let i = choose|i: int| 0 <= i < a.drop_first().len() && a.drop_first()[i] == h;
            assert(a[i + 1] == h); assert(a.contains(h));
        }
        if a.to_set().remove(a[0]).contains(h) {
            let i = choose|i: int| 0 <= i < a.len() && a[i] == h;
            assert(i > 0); assert(a.drop_first()[i - 1] == h); assert(a.drop_first().contains(h));
        }
    }
    assert(a.drop_first().to_set() =~= a.to_set().remove(a[0]));
}

/// Strictly increasing lists with the same members have the same exact order.
pub(crate) proof fn sorted_unique(a: Seq<CommitHash>, b: Seq<CommitHash>)
    requires a.to_set() == b.to_set(), a.no_duplicates(), b.no_duplicates(),
        vstd::std_specs::btree::increasing_seq(a), vstd::std_specs::btree::increasing_seq(b),
    ensures a == b,
    decreases a.len(),
{
    super::stdlib::commit_order_model();
    vstd::std_specs::btree::axiom_increasing_seq_meaning(a);
    vstd::std_specs::btree::axiom_increasing_seq_meaning(b);
    reveal(vstd::laws_cmp::obeys_cmp_ord);
    reveal(vstd::laws_cmp::obeys_partial_cmp_spec_properties);
    if a.len() == 0 { if b.len() > 0 { assert(b.to_set().contains(b[0])); assert(a.contains(b[0])); } assert(b.len() == 0); assert(a =~= b); }
    else {
        assert(a.to_set().contains(a[0])); assert(b.contains(a[0]));
        assert(b.len() > 0);
        assert(b.to_set().contains(b[0])); assert(a.contains(b[0]));
        let i = choose|i: int| 0 <= i < a.len() && a[i] == b[0];
        let j = choose|j: int| 0 <= j < b.len() && b[j] == a[0];
        if a[0] != b[0] {
            assert(i > 0 && j > 0);
            assert(a[0].cmp_spec(&a[i]) is Less); assert(b[0].cmp_spec(&b[j]) is Less);
            assert(a[0].partial_cmp_spec(&b[0]) == Some(core::cmp::Ordering::Less));
            assert(b[0].partial_cmp_spec(&a[0]) == Some(core::cmp::Ordering::Less));
            assert(b[0].partial_cmp_spec(&a[0]) == Some(core::cmp::Ordering::Greater));
            assert(false);
        }
        assert(a[0] == b[0]);
        tail_set(a); tail_set(b);
        assert(a.drop_first().to_set() =~= a.to_set().remove(a[0]));
        assert(b.drop_first().to_set() =~= b.to_set().remove(b[0]));
        vstd::std_specs::btree::axiom_increasing_seq_meaning(a.drop_first());
        vstd::std_specs::btree::axiom_increasing_seq_meaning(b.drop_first());
        sorted_unique(a.drop_first(), b.drop_first());
        assert forall|k: int| 0 <= k < a.len() implies a[k] == b[k] by {
            if k > 0 { assert(a[k] == a.drop_first()[k - 1]); assert(b[k] == b.drop_first()[k - 1]); }
        }
        assert(a =~= b);
    }
}

proof fn translated_bound(history: &History, checkpoint: &super::checkpoint::Checkpoint, records: Seq<super::change_plan::Change>)
    ensures events::translated(history, checkpoint, records).len() <= records.len(),
    decreases records.len(),
{
    if records.len() > 0 { translated_bound(history, checkpoint, records.drop_last()); }
}

impl Materialization {
    pub closed spec fn current<O, Q>(&self, ledger: &Ledger<O, Q>) -> bool { self.current_details(ledger) }
    pub closed spec fn planning_frame(&self, before: &Self) -> bool { self.planning_frame_details(before) }
    pub closed spec fn plan_outcome<O, Q>(&self, before: &Self, ledger: &Ledger<O, Q>, budget: usize, out: Result<Option<Batch>, Error>) -> bool {
        self.plan_outcome_details(before, ledger, budget, out)
    }
    pub closed spec fn plan_errors<O, Q>(&self, before: &Self, ledger: &Ledger<O, Q>, budget: usize, out: Result<Option<Batch>, Error>) -> bool {
        self.plan_errors_details(before, ledger, budget, out)
    }
    pub(crate) proof fn current_facts<O, Q>(&self, ledger: &Ledger<O, Q>)
        ensures self.current(ledger) == self.current_details(ledger),
    {}
    pub(crate) proof fn plan_facts<O, Q>(&self, before: &Self, ledger: &Ledger<O, Q>, budget: usize, out: Result<Option<Batch>, Error>)
        requires self.plan_outcome(before, ledger, budget, out), self.plan_errors(before, ledger, budget, out),
        ensures self.plan_outcome_details(before, ledger, budget, out), self.plan_errors_details(before, ledger, budget, out),
            self.planning_frame_details(before),
    {}

    pub(crate) open spec fn current_details<Options, Queue>(&self, ledger: &Ledger<Options, Queue>) -> bool {
        !self.poisoned && self.protocol.pending_value() is None && self.checkpoint.progress_view() is None
            && (self.owner is None || self.owner == Some(ledger.history_view().history_id()))
            && self.checkpoint.cursor_view().delivery_view().to_set() == ledger.history_view().delivered_heads_set()
            && self.checkpoint.cursor_view().delivery_view().no_duplicates()
            && vstd::std_specs::btree::increasing_seq(self.checkpoint.cursor_view().delivery_view())
            && ledger.history_view().follow_target(&self.checkpoint.follow_view(), &ledger.permissions_view(),
                &self.checkpoint.cursor_view().target_view())
    }

    pub fn is_current<Options, Queue>(&self, ledger: &Ledger<Options, Queue>) -> (out: bool)
        ensures out == self.current(ledger),
    {
        proof { self.representation(); self.checkpoint.representation(); }
        if self.poisoned || self.protocol.has_pending() || self.checkpoint.is_partial() { return false; }
        let history = ledger.history();
        if let Some(id) = self.owner { if id != history.id() { return false; } }
        let delivery = history.captured_delivery_heads();
        proof {
            if self.current(ledger) { sorted_unique(self.checkpoint.cursor_view().delivery_view(), delivery@); }
        }
        if !super::write_permission::same_heads(self.checkpoint.cursor.delivery_heads(), &delivery) { return false; }
        let target = ledger.view(self.follow());
        proof {
            let saved = self.checkpoint.cursor_view();
            saved.target_view().permissions_view().same_comparison(&target.permissions_view(), &ledger.permissions_view());
            if self.current(ledger) {
                match self.checkpoint.follow_view() {
                    super::checkpoint::Follow::Pinned(_) => {},
                    _ => sorted_unique(saved.target_view().heads_view(), target.heads_view()),
                }
            }
        }
        self.checkpoint.cursor.view_point() == &target
    }
    /// The published checkpoint and application-facing flags do not advance during planning.
    pub(crate) open spec fn planning_frame_details(&self, before: &Self) -> bool {
        self.checkpoint == before.checkpoint && self.dirty == before.dirty && self.poisoned == before.poisoned
            && self.last == before.last && self.supports_visibility_changes == before.supports_visibility_changes
            && self.protocol.planner_id() == before.protocol.planner_id()
    }

    pub(crate) open spec fn chosen_source(&self, history: &History, permissions: &WritePermissions,
        source: &super::materialization_source::Source) -> bool {
        if self.source is Some && !self.source.unwrap().finished() { *source == self.source.unwrap() }
        else { exists|input: super::materialization_start::Inputs|
            input.matches(history, &self.checkpoint, permissions, self.supports_visibility_changes) && input.started(history, source) }
    }

    pub(crate) open spec fn issued_from(&self, before: &Self, history: &History, permissions: &WritePermissions,
        budget: usize, batch: &Batch, origin: super::materialization_source::Source, records: Seq<super::change_plan::Change>) -> bool {
        &&& before.chosen_source(history, permissions, &origin)
        &&& origin.ready() && origin.history_id() == history.history_id()
        &&& self.source is Some && self.source.unwrap().ready()
        &&& self.source.unwrap().history_id() == history.history_id()
        &&& self.source.unwrap().base() == origin.base() && self.source.unwrap().target() == origin.target()
        &&& self.source.unwrap().delivery() == origin.delivery() && self.source.unwrap().total() == origin.total()
        &&& self.source.unwrap().delivered_members() == origin.delivered_members()
        &&& self.source.unwrap().before_members() == origin.before_members()
        &&& self.source.unwrap().after_members() == origin.after_members()
        &&& self.source.unwrap().processed() == origin.processed() + records.len()
        &&& self.source.unwrap().selected() == origin.selected().difference(super::change_plan::hashes(records).to_set())
        &&& self.source.unwrap().finished() == self.source.unwrap().selected().is_empty()
        &&& self.source.unwrap().processed() + self.source.unwrap().selected().len() == self.source.unwrap().total()
        &&& !self.source.unwrap().reset_pending()
        &&& records.len() == vstd::math::min(origin.selected().len() as int, if origin.reset_pending() { budget as int - 1 } else { budget as int })
        &&& super::change_plan::scheduled(history, origin.selected(), super::change_plan::hashes(records))
        &&& super::change_plan::hashes(records).no_duplicates()
        &&& forall|i: int| 0 <= i < records.len() ==> super::change_plan::mapped(history, records[i], origin.delivered_members(), origin.before_members(), origin.after_members())
        &&& batch.events_view() == (if origin.reset_pending() { seq![EventView::Reset] } else { Seq::empty() }) + events::translated(history, &before.checkpoint, records)
    }

    pub(crate) open spec fn plan_outcome_details<Options, Queue>(&self, before: &Self, ledger: &Ledger<Options, Queue>,
        budget: usize, out: Result<Option<Batch>, Error>) -> bool {
        let history = ledger.history_view();
        &&& self.planning_frame(before)
        &&& match out {
            Err(_) => self.protocol == before.protocol,
            Ok(None) => budget > 0 && self.owner == Some(history.history_id()) && self.current(ledger)
                && self.source is None && self.protocol == before.protocol,
            Ok(Some(batch)) => budget > 0 && self.owner == Some(history.history_id())
                && before.protocol.pending_value() is None && self.protocol.accepts(&batch) && batch.events_view().len() <= budget
                && self.protocol.sequence() == before.protocol.sequence() + 1
                && self.protocol.pending_events() == batch.events_view()
                && (exists|origin: super::materialization_source::Source, records: Seq<super::change_plan::Change>|
                    self.issued_from(before, &history, &ledger.permissions_view(), budget, &batch, origin, records))
                && self.source is Some && self.source.unwrap().ready()
                && (forall|i: int| #![trigger batch.events_view()[i]] 0 <= i < batch.events_view().len()
                    && events::activation(batch.events_view()[i]) is Some ==>
                    self.source.unwrap().after_members().contains(events::activation(batch.events_view()[i]).unwrap())
                        && !events::rejected(&before.checkpoint, &history, events::activation(batch.events_view()[i]).unwrap()))
                && self.pending_checkpoint_matches_source()
        }
    }

    pub(crate) open spec fn pending_checkpoint_matches_source(&self) -> bool {
        let source = self.source.unwrap();
        let pending = self.protocol.pending_value().unwrap();
        &&& self.protocol.pending_value() is Some && self.source is Some
        &&& pending.next.follow_view().same(&self.checkpoint.follow_view())
        &&& pending.next.rejected_view() == self.checkpoint.rejected_view()
        &&& (pending.next.progress_view() is None) == source.finished()
        &&& if source.finished() {
            source.processed() == source.total() && pending.next.cursor_view().delivery_view() == source.delivery()
                && pending.next.cursor_view().target_view().same(&source.target())
        } else {
            pending.next.cursor_view().same(&source.base()) && pending.next.progress_view() is Some
                && pending.next.progress_view().unwrap().target.same(&source.target())
                && pending.next.progress_view().unwrap().delivery_heads@ == source.delivery()
                && pending.next.progress_view().unwrap().applied == source.processed()
        }
        &&& pending.origin@ == pending.counts && pending.original_active@ == pending.active@
        &&& pending.corrected@.is_empty() && !pending.restart
        &&& pending.delivered@ == events::deliveries(self.protocol.pending_events())
        &&& pending.active@ == events::active_deliveries(self.protocol.pending_events())
        &&& pending.counts.delivered == events::count(self.protocol.pending_events(), 1)
        &&& pending.counts.activated == events::count(self.protocol.pending_events(), 2)
        &&& pending.counts.retracted == events::count(self.protocol.pending_events(), 3)
        &&& pending.counts.rejected == 0 && pending.counts.rebuilt == (events::count(self.protocol.pending_events(), 0) > 0)
    }

    pub(crate) open spec fn guard_valid(&self, history: &History) -> bool {
        !self.poisoned && (self.owner is None || self.owner == Some(history.history_id()))
            && (self.owner is Some || super::materialization_driver::binding_known(history, &self.checkpoint))
    }

    pub(crate) open spec fn guard_error(&self, history: &History, error: Error) -> bool {
        if self.poisoned { error is Poisoned }
        else if self.owner is Some && self.owner != Some(history.history_id()) { error is WrongHistory }
        else { self.owner is None && !super::materialization_driver::binding_known(history, &self.checkpoint)
            && error is UnknownCommit && !history.known(error->UnknownCommit_0)
            && super::materialization_driver::binding_references(&self.checkpoint).contains(error->UnknownCommit_0) }
    }

    pub(crate) open spec fn startup_error(&self, history: &History, permissions: &WritePermissions, error: Error) -> bool {
        (self.source is None || self.source.unwrap().finished())
            && (super::materialization_start::input_error(history, &self.checkpoint, self.supports_visibility_changes, error)
                || exists|input: super::materialization_start::Inputs|
                    input.matches(history, &self.checkpoint, permissions, self.supports_visibility_changes)
                        && input.construction_error(history, error))
    }

    pub(crate) open spec fn plan_errors_details<Options, Queue>(&self, before: &Self, ledger: &Ledger<Options, Queue>,
        budget: usize, out: Result<Option<Batch>, Error>) -> bool {
        let history = ledger.history_view();
        if !before.guard_valid(&history) { out is Err && before.guard_error(&history, out->Err_0) && *self == *before }
        else {
            &&& self.owner == Some(history.history_id())
            &&& (budget > 0 && before.protocol.pending_value() is None
                && (before.source is None || before.source.unwrap().finished()) && before.current(ledger) ==> out == Ok(None))
            &&& if budget == 0 { out is Err && out->Err_0 is Limit && self.same_except_owner(before) }
                else if before.protocol.pending_value() is Some { out is Err && out->Err_0 is Protocol && self.same_except_owner(before) }
                else { match out {
                    Ok(_) => true,
                    Err(error) => match error {
                        Error::Exhausted(_) => before.protocol.sequence() == u64::MAX && self.source is Some && self.source.unwrap().ready(),
                        Error::Poisoned => before.source is Some && !before.source.unwrap().finished()
                            && !before.source.unwrap().ready() && self.source == before.source,
                        Error::WrongHistory => before.source is Some && !before.source.unwrap().finished()
                            && before.source.unwrap().ready() && before.source.unwrap().history_id() != history.history_id() && self.source == before.source,
                        _ => before.startup_error(&history, &ledger.permissions_view(), error) && self.source is None,
                    }
                } }
        }
    }

    /// Plan at most `budget` source records (a reset consumes one slot).
    /// Payload bytes stay in your storage. Zero budget is an error.
    pub fn plan<Options, Queue>(&mut self, ledger: &Ledger<Options, Queue>, budget: usize)
        -> (out: Result<Option<Batch>, Error>)
        ensures old(self).counters_consistent() ==> final(self).counters_consistent(),
            old(self).checkpoint_consistent() ==> final(self).checkpoint_consistent(),
            old(self).source_consistent() ==> final(self).source_consistent(),
            old(self).source_consistent() && (out matches Err(Error::Poisoned)) ==> old(self).poisoned_view(),
            old(self).source_consistent() && (out matches Err(Error::WrongHistory)) ==>
                old(self).owner_view() is Some && old(self).owner_view().unwrap() != ledger.history_view().history_id(),
            final(self).plan_outcome(old(self), ledger, budget, out),
            final(self).plan_errors(old(self), ledger, budget, out),
    {
        proof { Materialization::counter_consistency_definition(); Materialization::checkpoint_consistency_definition(); Materialization::source_consistency_definition(); }
        let ghost before = *self;
        proof { before.current_facts(ledger); }
        let history = ledger.history();
        let permissions = ledger.permissions();
        self.check_history(history, permissions)?;
        if budget == 0 { return Err(Error::Limit("materialization budget must be positive")); }
        if self.protocol.has_pending() {
            return Err(Error::Protocol("complete the outstanding batch before planning"));
        }
        let done = match &self.source { Some(source) => source.is_done(), None => false };
        if done { self.source = None; }
        proof { self.current_facts(ledger); }
        if self.source.is_none() {
            if self.is_current(ledger) { return Ok(None); }
            self.start_history(history, permissions)?;
        }
        let ghost origin = self.source.unwrap();
        assert(before.chosen_source(history, permissions, &origin));
        let source = self.source.as_mut().unwrap();
        let emitted = source.advance(history, budget)?;
        let (translated, summary) = events::translate(history, &self.checkpoint, &emitted.records, emitted.reset);
        proof { events::translation_authorized(history, &self.checkpoint, emitted.records@, origin.after_members());
            translated_bound(history, &self.checkpoint, emitted.records@); }
        proof {
            super::materialization_corrections::translated_active_facts(history,&self.checkpoint,emitted.records@,emitted.reset);
            super::materialization_invariants::translated_inventory(history,&self.checkpoint,emitted.records@,emitted.reset);
            super::materialization_invariants::active_inventory(super::batch_protocol::event_views(translated@));
        }
        let next = source.checkpoint(&self.checkpoint)?;
        let batch = self.protocol.install(Pending::new(next, summary), translated)?;
        assert(self.issued_from(&before, history, permissions, budget, &batch, origin, emitted.records@));
        assert forall|i: int| #![trigger batch.events_view()[i]] 0 <= i < batch.events_view().len()
            && events::activation(batch.events_view()[i]) is Some implies
            self.source.unwrap().after_members().contains(events::activation(batch.events_view()[i]).unwrap())
                && !events::rejected(&before.checkpoint, history, events::activation(batch.events_view()[i]).unwrap()) by {
            let offset = if emitted.reset { 1int } else { 0int };
            if i >= offset { assert(batch.events_view()[i] == events::translated(history, &before.checkpoint, emitted.records@)[i - offset]); }
        }
        assert(self.pending_checkpoint_matches_source());
        assert(self.protocol.pending_value().unwrap().next.rejected_view().dom().difference(self.checkpoint.rejected_view().dom()) =~= Set::<CommitHash>::empty());
        assert(super::materialization_invariants::pending_aligned(&self.protocol.pending_value().unwrap(),&self.source.unwrap(),&self.checkpoint));
        assert(super::materialization_invariants::counted(&self.protocol.pending_value().unwrap(),&self.source.unwrap()));
        Ok(Some(batch))
    }
}

} // verus!
