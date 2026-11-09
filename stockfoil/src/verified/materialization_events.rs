//! Translate captured changes into application events using actual history metadata and rejections.
use super::{
    author::Author, batch_protocol::Event, change_plan::Change, checkpoint::Checkpoint,
    commit_hash::CommitHash, events::Vis, history::History, history_data::PayloadId,
};
use std::collections::HashSet;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

#[cfg_attr(not(verus_keep_ghost), derive(Clone))]
pub struct Counts {
    pub delivered: usize,
    pub activated: usize,
    pub retracted: usize,
    pub rejected: usize,
    pub rebuilt: bool,
}
impl Default for Counts {
    fn default() -> (out: Self)
        ensures out.delivered == 0, out.activated == 0, out.retracted == 0, out.rejected == 0, !out.rebuilt,
    { Self { delivered: 0, activated: 0, retracted: 0, rejected: 0, rebuilt: false } }
}

impl History {
    pub(crate) open spec fn event_author(&self, hash: CommitHash) -> Author {
        self.owners_view()[self.read_state().labels[self.read_state().hashes[hash]@ as int] as int]
    }
    pub(crate) open spec fn event_parents(&self, hash: CommitHash) -> Seq<CommitHash> {
        let nodes = self.topology.nodes_view();
        super::storage::parents(nodes[self.read_state().hashes[hash]@ as int], self.topology.ids_view())
            .map(|i: int, p: super::clock::NodeIdx| nodes[p@ as int].hash)
    }
    pub(crate) fn event_metadata(&self, hash: &CommitHash) -> (out: (Author, Vec<CommitHash>))
        requires self.known(*hash),
        ensures out.0 == self.event_author(*hash), out.1@ == self.event_parents(*hash),
    {
        proof { self.representation(); self.topology.established(); self.topology.metadata_facts(); }
        let idx = self.topology.by_hash().get(hash).unwrap();
        let nodes = self.topology.nodes();
        let node = &nodes[idx.index()];
        let author = self.topology.chain_authors()[node.chain as usize];
        let ids = self.topology.parent_ids();
        let mut parents = Vec::with_capacity(node.parents_len as usize);
        let mut i = 0usize;
        while i < node.parents_len as usize
            invariant i <= node.parents_len, parents.len() == i,
                nodes@ == self.topology.nodes_view(), ids@ == self.topology.ids_view(),
                *node == nodes[self.read_state().hashes[*hash]@ as int],
                node.parents_start + node.parents_len <= ids.len(),
                forall|j: int| node.parents_start <= j < node.parents_start + node.parents_len ==> ids[j]@ < nodes.len(),
                forall|j: int| 0 <= j < i ==> parents[j] == self.event_parents(*hash)[j],
            decreases node.parents_len - i,
        {
            parents.push(nodes[ids[node.parents_start + i].index()].hash);
            i += 1;
        }
        assert(parents@ =~= self.event_parents(*hash));
        (author, parents)
    }
}

pub(crate) open spec fn rejected(checkpoint: &Checkpoint, history: &History, hash: CommitHash) -> bool {
    checkpoint.rejected_view().contains_key(hash) || exists|root: CommitHash|
        checkpoint.rejected_view().contains_key(root) && checkpoint.rejected_view()[root]
            && history.strict_ancestor(root, hash)
}

pub(crate) fn is_rejected(checkpoint: &Checkpoint, history: &History, hash: &CommitHash) -> (out: bool)
    ensures out == rejected(checkpoint, history, *hash),
{
    proof { checkpoint.representation(); super::stdlib::commit_order_model(); }
    if checkpoint.rejected.contains_key(hash) { return true; }
    for (root, cascade) in iter: checkpoint.rejected.iter()
        invariant !checkpoint.rejected_view().contains_key(*hash),
            forall|j: int| 0 <= j < iter.seq().len() ==> checkpoint.rejected_view().contains_key(*iter.seq()[j].0)
                && checkpoint.rejected_view()[*iter.seq()[j].0] == *iter.seq()[j].1,
            forall|root: CommitHash| checkpoint.rejected_view().contains_key(root) ==> iter.seq().contains((&root, &checkpoint.rejected_view()[root])),
            forall|j: int| 0 <= j < iter.index() ==> !(*iter.seq()[j].1 && history.strict_ancestor(*iter.seq()[j].0, *hash)),
    {
        if *cascade && history.is_ancestor(root, hash) { return true; }
    }
    false
}

pub(crate) open spec fn record_event(history: &History, checkpoint: &Checkpoint, record: Change) -> Seq<super::batch_protocol::EventView<PayloadId>> {
    use super::batch_protocol::EventView;
    if record.newly_delivered {
        seq![EventView::Deliver { hash: record.hash, author: history.event_author(record.hash),
            parents: history.event_parents(record.hash), payload: record.payload,
            active: !rejected(checkpoint, history, record.hash) && super::events::after(record.visibility) }]
    } else if rejected(checkpoint, history, record.hash) { Seq::empty() }
    else { match record.visibility {
        Vis::After => seq![EventView::Activate { hash: record.hash }],
        Vis::Before => seq![EventView::Retract { hash: record.hash }],
        _ => Seq::empty(),
    } }
}

pub(crate) open spec fn translated(history: &History, checkpoint: &Checkpoint, records: Seq<Change>) -> Seq<super::batch_protocol::EventView<PayloadId>>
    decreases records.len(),
{
    if records.len() == 0 { Seq::empty() }
    else { translated(history, checkpoint, records.drop_last()) + record_event(history, checkpoint, records.last()) }
}

pub(crate) open spec fn activation<P>(event: super::batch_protocol::EventView<P>) -> Option<CommitHash> {
    match event {
        super::batch_protocol::EventView::Deliver { hash, active: true, .. }
        | super::batch_protocol::EventView::Activate { hash } => Some(hash),
        _ => None,
    }
}

/// Event translation can suppress participation, but cannot grant new participation.
pub(crate) proof fn translation_authorized(history: &History, checkpoint: &Checkpoint,
    records: Seq<Change>, authorized: Set<CommitHash>)
    requires forall|i: int| 0 <= i < records.len() && super::events::after(records[i].visibility) ==> authorized.contains(records[i].hash),
    ensures forall|i: int| #![trigger translated(history, checkpoint, records)[i]] 0 <= i < translated(history, checkpoint, records).len() ==> {
        let event = translated(history, checkpoint, records)[i];
        activation(event) is Some ==> authorized.contains(activation(event).unwrap())
            && !rejected(checkpoint, history, activation(event).unwrap())
    },
    decreases records.len(),
{
    if records.len() > 0 {
        translation_authorized(history, checkpoint, records.drop_last(), authorized);
        assert forall|i: int| #![trigger translated(history, checkpoint, records)[i]] 0 <= i < translated(history, checkpoint, records).len() implies {
            let event = translated(history, checkpoint, records)[i];
            activation(event) is Some ==> authorized.contains(activation(event).unwrap())
                && !rejected(checkpoint, history, activation(event).unwrap())
        } by {
            let prefix = translated(history, checkpoint, records.drop_last());
            if i >= prefix.len() {
                assert(super::events::after(records.last().visibility) ==> authorized.contains(records.last().hash));
            }
        }
    }
}

pub(crate) struct Summary {
    pub counts: Counts,
    pub delivered: HashSet<CommitHash>,
    pub active: Vec<CommitHash>,
}

pub(crate) open spec fn kind(event: super::batch_protocol::EventView<PayloadId>) -> int {
    use super::batch_protocol::EventView;
    match event { EventView::Reset => 0, EventView::Deliver { .. } => 1,
        EventView::Activate { .. } => 2, EventView::Retract { .. } => 3, EventView::Payload { .. } => 4 }
}
pub(crate) open spec fn count(events: Seq<super::batch_protocol::EventView<PayloadId>>, tag: int) -> nat
    decreases events.len(),
{
    if events.len() == 0 { 0 }
    else { count(events.drop_last(), tag) + if kind(events.last()) == tag { 1nat } else { 0nat } }
}
pub(crate) open spec fn deliveries(events: Seq<super::batch_protocol::EventView<PayloadId>>) -> Set<CommitHash>
    decreases events.len(),
{
    if events.len() == 0 { Set::empty() }
    else { match events.last() {
        super::batch_protocol::EventView::Deliver { hash, .. } => deliveries(events.drop_last()).insert(hash),
        _ => deliveries(events.drop_last()),
    } }
}
pub(crate) open spec fn active_deliveries(events: Seq<super::batch_protocol::EventView<PayloadId>>) -> Seq<CommitHash>
    decreases events.len(),
{
    if events.len() == 0 { Seq::empty() }
    else { match events.last() {
        super::batch_protocol::EventView::Deliver { hash, active: true, .. } => active_deliveries(events.drop_last()).push(hash),
        _ => active_deliveries(events.drop_last()),
    } }
}
impl Summary {
    pub(crate) open spec fn describes(&self, events: Seq<super::batch_protocol::EventView<PayloadId>>) -> bool {
        self.delivered@ == deliveries(events) && self.active@ == active_deliveries(events)
            && self.counts.delivered == count(events, 1) && self.counts.activated == count(events, 2)
            && self.counts.retracted == count(events, 3) && self.counts.rebuilt == (count(events, 0) > 0)
            && self.counts.rejected == 0
    }
}
pub(crate) fn summarize(events: &[Event<PayloadId>]) -> (out: Summary)
    ensures out.describes(super::batch_protocol::event_views(events@)),
{
    proof { super::stdlib::commit_key_model(); }
    let mut out = Summary { counts: Counts::default(), delivered: HashSet::new(), active: Vec::new() };
    let mut i = 0usize;
    while i < events.len()
        invariant i <= events.len(),
            out.describes(super::batch_protocol::event_views(events@.take(i as int))),
            out.counts.delivered <= i, out.counts.activated <= i, out.counts.retracted <= i,
            vstd::std_specs::hash::obeys_key_model::<CommitHash>(),
        decreases events.len() - i,
    {
        match &events[i] {
            Event::Reset => { out.counts.rebuilt = true; },
            Event::Deliver { hash, active, .. } => {
                out.counts.delivered += 1;
                out.delivered.insert(*hash);
                if *active { out.active.push(*hash); }
            },
            Event::Activate { .. } => { out.counts.activated += 1; },
            Event::Retract { .. } => { out.counts.retracted += 1; },
            Event::Payload { .. } => {},
        }
        assert(super::batch_protocol::event_views(events@.take(i + 1)).drop_last() =~= super::batch_protocol::event_views(events@.take(i as int)));
        i += 1;
    }
    assert(events@.take(i as int) =~= events@);
    out
}

pub(crate) fn translate(history: &History, checkpoint: &Checkpoint, records: &[Change], reset: bool) -> (out: (Vec<Event<PayloadId>>, Summary))
    requires forall|i: int| 0 <= i < records.len() ==> history.known(records[i].hash),
    ensures super::batch_protocol::event_views(out.0@) == (if reset { seq![super::batch_protocol::EventView::Reset] } else { Seq::empty() }) + translated(history, checkpoint, records@),
        out.1.describes(super::batch_protocol::event_views(out.0@)),
{
    let mut events = Vec::new();
    if reset { events.push(Event::Reset); }
    let mut i = 0usize;
    while i < records.len()
        invariant i <= records.len(),
            forall|j: int| 0 <= j < records.len() ==> history.known(records[j].hash),
            super::batch_protocol::event_views(events@) == (if reset { seq![super::batch_protocol::EventView::Reset] } else { Seq::empty() }) + translated(history, checkpoint, records@.take(i as int)),
        decreases records.len() - i,
    {
        let ghost previous = super::batch_protocol::event_views(events@);
        let record = &records[i];
        let rejected = is_rejected(checkpoint, history, &record.hash);
        if record.newly_delivered {
            let (author, parents) = history.event_metadata(&record.hash);
            events.push(Event::Deliver { hash: record.hash, author, parents, payload: record.payload,
                active: !rejected && matches!(record.visibility, Vis::After | Vis::Both) });
        } else if !rejected {
            match record.visibility {
                Vis::After => { events.push(Event::Activate { hash: record.hash }); },
                Vis::Before => { events.push(Event::Retract { hash: record.hash }); },
                _ => {},
            }
        }
        assert(super::batch_protocol::event_views(events@) =~= previous + record_event(history, checkpoint, *record));
        assert(records@.take(i + 1).drop_last() =~= records@.take(i as int));
        i += 1;
    }
    assert(records@.take(i as int) =~= records@);
    let summary = summarize(&events);
    (events, summary)
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verified::{
        materialization_source::Source,
        view::{MaterializationCursor, ViewPoint},
    };
    use crate::{Follow, WritePermission, WritePermissions};

    #[test]
    fn captured_delivery_retains_metadata_but_cascade_suppresses_activity() {
        let [root, child, other, join] = [1, 2, 3, 4].map(|n| CommitHash([n; 32]));
        let authors = [1, 2, 3, 4].map(|n| Author([n; 32]));
        let mut history = History::new();
        for (hash, author, parents, payload) in [
            (root, authors[0], vec![], 10),
            (child, authors[1], vec![root], 20),
            (other, authors[2], vec![], 30),
            (join, authors[3], vec![other, child, other], 40),
        ] {
            history
                .topology
                .insert(hash, author, &parents, payload, 0)
                .unwrap();
        }
        let mut checkpoint = Checkpoint::new(Follow::Working);
        checkpoint.reject(root, false);
        assert!(is_rejected(&checkpoint, &history, &root));
        assert!(!is_rejected(&checkpoint, &history, &child));
        checkpoint.reject(root, true);
        checkpoint.reject(root, false); // A later direct rejection cannot weaken a cascade.
        let missing = CommitHash([99; 32]);
        checkpoint.reject(missing, true);
        assert!(is_rejected(&checkpoint, &history, &missing));
        assert!(!is_rejected(&checkpoint, &history, &other));
        let target = ViewPoint::from_parts(
            vec![join],
            WritePermissions::new().with_unlisted(WritePermission::Unbounded),
        );
        let mut source = Source::new(
            &history,
            MaterializationCursor::new(),
            target,
            vec![join],
            0,
            true,
        )
        .unwrap();
        let batch = source.advance(&history, 100).unwrap();
        let (events, summary) = translate(&history, &checkpoint, &batch.records, batch.reset);
        assert_eq!(
            events,
            vec![
                Event::Reset,
                Event::Deliver {
                    hash: root,
                    author: authors[0],
                    parents: vec![],
                    payload: 10,
                    active: false
                },
                Event::Deliver {
                    hash: child,
                    author: authors[1],
                    parents: vec![root],
                    payload: 20,
                    active: false
                },
                Event::Deliver {
                    hash: other,
                    author: authors[2],
                    parents: vec![],
                    payload: 30,
                    active: true
                },
                Event::Deliver {
                    hash: join,
                    author: authors[3],
                    parents: vec![other, child, other],
                    payload: 40,
                    active: false
                },
            ]
        );
        assert_eq!(summary.delivered, HashSet::from([root, child, other, join]));
        assert_eq!(summary.active, vec![other]);
        assert_eq!(
            (
                summary.counts.delivered,
                summary.counts.activated,
                summary.counts.retracted,
                summary.counts.rejected,
                summary.counts.rebuilt
            ),
            (4, 0, 0, 0, true)
        );
    }

    #[test]
    fn captured_visibility_switch_filters_rejected_records_and_retains_order() {
        let hashes = [1, 2, 3, 4].map(|n| CommitHash([n; 32]));
        let authors = [1, 2, 3, 4].map(|n| Author([n; 32]));
        let mut history = History::new();
        for i in 0..4 {
            history
                .topology
                .insert(hashes[i], authors[i], &[], i as u64, 0)
                .unwrap();
        }
        let before = ViewPoint::from_parts(
            vec![hashes[0], hashes[2]],
            WritePermissions::new().with_unlisted(WritePermission::Unbounded),
        );
        let after = ViewPoint::from_parts(
            vec![hashes[1], hashes[3]],
            WritePermissions::new().with_unlisted(WritePermission::Unbounded),
        );
        let base = MaterializationCursor::from_parts(hashes.to_vec(), before);
        let mut checkpoint = Checkpoint::default();
        checkpoint.reject(hashes[2], false);
        checkpoint.reject(hashes[3], false);
        let mut source = Source::new(&history, base, after, hashes.to_vec(), 0, false).unwrap();
        let batch = source.advance(&history, 100).unwrap();
        assert_eq!(batch.records.len(), 4);
        let (events, summary) = translate(&history, &checkpoint, &batch.records, batch.reset);
        assert_eq!(
            events,
            vec![
                Event::Retract { hash: hashes[0] },
                Event::Activate { hash: hashes[1] }
            ]
        );
        assert!(summary.delivered.is_empty());
        assert!(summary.active.is_empty()); // This is the list of active *new deliveries* for cascade correction.
        assert_eq!(
            (
                summary.counts.delivered,
                summary.counts.activated,
                summary.counts.retracted,
                summary.counts.rebuilt
            ),
            (0, 1, 1, false)
        );
        let next = source.checkpoint(&checkpoint).unwrap();
        assert!(!next.is_partial()); // Progress counts suppressed records too.
    }
}
