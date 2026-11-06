//! State-independent materialization planning and its in-memory driver.
use crate::history::checkpoint::InProgress;
use crate::history::view;
use crate::{
    Author, Bytes, ChangeBatch, Changes, Checkpoint, Codec, CommitHash, Document, Draft,
    DriveError, Error, Follow, Heads, History, Ledger, MaterializationCursor, PayloadId, ViewPoint,
    Vis, WritePermission,
};
use std::collections::{BTreeSet, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_PLANNER: AtomicU64 = AtomicU64::new(1);

/// Incremental view planner that brings an application state in sync with a
/// [`Ledger`]'s history and [`WritePermissions`](crate::WritePermissions).
///
/// A `Materialization` owns its [`Follow`] rule, permanent payload rejections,
/// and durable [`Checkpoint`], without owning the application state itself or
/// performing I/O.
///
/// - **In-memory states** implement [`Materializer`] and drive updates via
///   [`Replica`](crate::Replica) or [`Materialization::update`] /
///   [`Materialization::step`].
/// - **Out-of-core / async states** call [`plan`](Self::plan) to get a
///   [`Batch`] of [`Event<PayloadId>`]s, apply them against their own storage,
///   and call [`complete`](Self::complete) until it returns `Ok(None)`, then
///   persist their state and [`checkpoint`](Self::checkpoint) atomically.
///
/// Every commit entering the graph is delivered via [`Event::Deliver`], even if
/// `active` is `false` (disallowed by [`WritePermissions`](crate::WritePermissions)
/// or outside a pinned view), because later active commits may reference its
/// structural metadata. When a permission change or branch switch alters
/// whether an already-delivered commit participates, the planner emits
/// [`Event::Activate`] and [`Event::Retract`]—or [`Event::Reset`] followed by a
/// clean replay if `supports_visibility_changes` is `false`.
pub struct Materialization {
    checkpoint: Checkpoint,
    supports_visibility_changes: bool,
    source: Option<Source>,
    pending: Option<Pending>,
    owner: Option<u64>,
    identity: u64,
    ticket: u64,
    dirty: bool,
    poisoned: bool,
    last: Counts,
}
struct Source {
    changes: Changes,
    base: MaterializationCursor,
    target: ViewPoint,
    delivery_heads: Vec<CommitHash>,
    processed: u64,
    reset: bool,
    done: Option<MaterializationCursor>,
}
struct Pending {
    ticket: u64,
    next: Checkpoint,
    delivered: HashSet<CommitHash>,
    active: Vec<CommitHash>,
    corrected: HashSet<CommitHash>,
    restart: bool,
    counts: Counts,
}
#[derive(Default, Clone)]
struct Counts {
    delivered: usize,
    activated: usize,
    retracted: usize,
    rejected: usize,
    rebuilt: bool,
}
impl Materialization {
    pub fn new(follow: Follow, supports_visibility_changes: bool) -> Self {
        Self::resume(Checkpoint::new(follow), supports_visibility_changes)
    }
    pub fn resume(checkpoint: Checkpoint, supports_visibility_changes: bool) -> Self {
        Self {
            checkpoint,
            supports_visibility_changes,
            source: None,
            pending: None,
            owner: None,
            identity: NEXT_PLANNER.fetch_add(1, Ordering::Relaxed),
            ticket: 0,
            dirty: false,
            poisoned: false,
            last: Counts::default(),
        }
    }
    pub fn checkpoint(&self) -> &Checkpoint {
        &self.checkpoint
    }
    pub fn follow(&self) -> &Follow {
        self.checkpoint.follow()
    }
    /// An interrupted traversal is finished before following the new rule.
    pub fn set_follow(&mut self, follow: Follow) {
        self.checkpoint.follow = follow;
    }
    /// Last completed view; partial states must not be labelled as that target.
    pub fn heads(&self) -> &[CommitHash] {
        self.checkpoint.heads()
    }
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
    pub fn is_current(&self, ledger: &Ledger) -> bool {
        !self.poisoned
            && self.pending.is_none()
            && !self.checkpoint.is_partial()
            && self.owner.is_none_or(|id| id == ledger.history().id())
            && self.checkpoint.cursor.delivery_heads() == ledger.heads(Heads::Delivered)
            && self.checkpoint.cursor.view_point() == &ledger.view(self.follow())
    }
    fn check(&mut self, ledger: &Ledger) -> Result<(), Error> {
        if self.poisoned {
            return Err(Error::Poisoned);
        }
        let id = ledger.history().id();
        if self.owner.is_some_and(|owner| owner != id) {
            return Err(Error::WrongHistory);
        }
        if self.owner.is_none() {
            // A durable checkpoint is graph-independent, but every named head
            // and rejection must be resolvable in the rebuilt graph.
            ledger.history().ancestry(
                self.checkpoint.cursor.delivery_heads(),
                ledger.permissions(),
            )?;
            ledger.history().ancestry(
                self.checkpoint.cursor.view_point().heads(),
                ledger.permissions(),
            )?;
            if let Follow::Pinned(heads) = self.follow() {
                ledger.ancestry(heads)?;
            }
            for hash in self.checkpoint.rejected.keys() {
                if !ledger.contains(hash) {
                    return Err(Error::UnknownCommit(*hash));
                }
            }
            self.owner = Some(id);
        }
        Ok(())
    }
    fn start(&mut self, ledger: &Ledger) -> Result<(), Error> {
        let history = ledger.history();
        let (mut base, target, delivery_heads, processed, reset) =
            match &self.checkpoint.in_progress {
                Some(progress) => (
                    self.checkpoint.cursor.clone(),
                    progress.target.clone(),
                    progress.delivery_heads.clone(),
                    progress.applied,
                    false,
                ),
                None => {
                    let target = ledger.view(self.follow());
                    let reset = !self.supports_visibility_changes
                        && view::visibility_changed(history, &self.checkpoint.cursor, &target)?;
                    (
                        self.checkpoint.cursor.clone(),
                        target,
                        ledger.heads(Heads::Delivered),
                        0,
                        reset,
                    )
                }
            };
        if reset {
            base = MaterializationCursor::new();
        }
        let mut changes = view::plan_changes_at(history, &base, &target, &delivery_heads)?;
        if processed > changes.remaining() as u64 {
            return Err(Error::Malformed(
                "checkpoint record count exceeds captured traversal",
            ));
        }
        let mut skipped = Vec::new();
        let mut left = processed as usize;
        while left > 0 {
            skipped.clear();
            let count = left.min(1024);
            history.next_changes(&mut changes, &mut skipped, count)?;
            left -= count;
        }
        self.source = Some(Source {
            changes,
            base,
            target,
            delivery_heads,
            processed,
            reset,
            done: None,
        });
        Ok(())
    }
    /// Plan at most `budget` source records (plus a reset event if necessary).
    /// Payload bytes stay in your storage. Zero budget is an error.
    pub fn plan(&mut self, ledger: &Ledger, budget: usize) -> Result<Option<Batch>, Error> {
        self.check(ledger)?;
        if budget == 0 {
            return Err(Error::Limit("materialization budget must be positive"));
        }
        if self.pending.is_some() {
            return Err(Error::Protocol(
                "complete the outstanding batch before planning",
            ));
        }
        if self.source.as_ref().is_some_and(|s| s.done.is_some()) {
            self.source = None;
        }
        if self.source.is_none() {
            if self.is_current(ledger) {
                return Ok(None);
            }
            self.start(ledger)?;
        }
        let source = self.source.as_mut().unwrap();
        let mut events = Vec::new();
        let mut counts = Counts::default();
        if source.reset {
            events.push(Event::Reset);
            source.reset = false;
            counts.rebuilt = true;
        }
        let mut records = Vec::new();
        let slots = budget.saturating_sub(events.len());
        let status = ledger
            .history()
            .next_changes(&mut source.changes, &mut records, slots)?;
        source.processed += records.len() as u64;
        if let ChangeBatch::Complete(cursor) = status {
            source.done = Some(cursor);
        }
        for record in records {
            let rejected = is_rejected(&self.checkpoint, ledger, &record.hash);
            if record.newly_delivered {
                events.push(Event::Deliver {
                    hash: record.hash,
                    author: ledger.author_of(&record.hash).unwrap(),
                    parents: ledger.parents(&record.hash).unwrap(),
                    payload: record.payload,
                    active: !rejected && matches!(record.visibility, Vis::After | Vis::Both),
                });
                counts.delivered += 1;
            } else if !rejected {
                match record.visibility {
                    Vis::After => {
                        events.push(Event::Activate { hash: record.hash });
                        counts.activated += 1;
                    }
                    Vis::Before => {
                        events.push(Event::Retract { hash: record.hash });
                        counts.retracted += 1;
                    }
                    _ => {}
                }
            }
        }
        let mut next = self.checkpoint.clone();
        if let Some(cursor) = &source.done {
            next.cursor = cursor.clone();
            next.in_progress = None;
        } else {
            next.cursor = source.base.clone();
            next.in_progress = Some(InProgress {
                target: source.target.clone(),
                delivery_heads: source.delivery_heads.clone(),
                applied: source.processed,
            });
        }
        let delivered = events
            .iter()
            .filter_map(|e| match e {
                Event::Deliver { hash, .. } => Some(*hash),
                _ => None,
            })
            .collect();
        let active = events
            .iter()
            .filter_map(|e| match e {
                Event::Deliver {
                    hash, active: true, ..
                } => Some(*hash),
                _ => None,
            })
            .collect();
        self.install(
            Pending {
                ticket: 0,
                next,
                delivered,
                active,
                corrected: HashSet::new(),
                restart: false,
                counts,
            },
            events,
        )
        .map(Some)
    }
    fn install(
        &mut self,
        mut pending: Pending,
        events: Vec<Event<PayloadId>>,
    ) -> Result<Batch, Error> {
        self.ticket = self
            .ticket
            .checked_add(1)
            .ok_or(Error::Exhausted("materialization tickets"))?;
        pending.ticket = self.ticket;
        let batch = Batch {
            rebuild: events.iter().any(|e| matches!(e, Event::Reset)),
            events,
            planner: self.identity,
            ticket: self.ticket,
        };
        self.pending = Some(pending);
        Ok(batch)
    }
    /// Record a batch's effects. The ledger is needed for requested payload IDs
    /// and cascading rejection ancestry; no borrow survives this call.
    /// Returns follow-up events until the checkpoint is safe to save. A newly
    /// rejected ancestor can retract descendants delivered in the same batch,
    /// or reset/replay a state that cannot retract them.
    pub fn complete(
        &mut self,
        ledger: &Ledger,
        batch: Batch,
        applied: Applied,
    ) -> Result<Option<Batch>, Error> {
        self.check(ledger)?;
        let pending = self.pending.as_ref().ok_or(Error::StaleBatch)?;
        // Batch contents are immutable through the public Rust API, so the
        // planner identity and ticket suffice. Bindings validate any mutable
        // foreign representation before returning the retained Rust token.
        if batch.planner != self.identity || batch.ticket != pending.ticket {
            return Err(Error::StaleBatch);
        }
        let mut seen = HashSet::new();
        for rejected in &applied.rejected {
            if !pending.delivered.contains(&rejected.hash) || !seen.insert(rejected.hash) {
                return Err(Error::Protocol(
                    "rejections must name distinct delivered commits from this batch",
                ));
            }
        }
        let wanted: BTreeSet<_> = applied.wanted.into_iter().collect();
        for hash in &wanted {
            if !ledger.contains(hash) {
                return Err(Error::UnknownCommit(*hash));
            }
        }
        let mut pending = self.pending.take().unwrap();
        pending.next.follow = self.checkpoint.follow.clone();
        for rejected in applied.rejected {
            let old = pending
                .next
                .rejected
                .insert(rejected.hash, rejected.cascade);
            if old == Some(true) {
                pending.next.rejected.insert(rejected.hash, true);
            }
            if old.is_none() {
                pending.counts.rejected += 1;
            }
        }
        let affected: Vec<_> = pending
            .active
            .iter()
            .copied()
            .filter(|h| {
                !pending.next.rejected.contains_key(h)
                    && !pending.corrected.contains(h)
                    && is_rejected(&pending.next, ledger, h)
            })
            .collect();
        let mut events = Vec::new();
        if !affected.is_empty() && !self.supports_visibility_changes {
            let source = self.source.as_ref().unwrap();
            pending.next.cursor = MaterializationCursor::new();
            pending.next.in_progress = Some(InProgress {
                target: source.target.clone(),
                delivery_heads: source.delivery_heads.clone(),
                applied: 0,
            });
            pending.restart = true;
            pending.active.clear();
            pending.counts.rebuilt = true;
            events.push(Event::Reset);
        } else {
            for hash in affected {
                pending.corrected.insert(hash);
                events.push(Event::Retract { hash });
                pending.counts.retracted += 1;
            }
            for hash in wanted {
                events.push(Event::Payload {
                    hash,
                    payload: ledger.payload_id(&hash).unwrap(),
                });
            }
        }
        if !events.is_empty() {
            return self.install(pending, events).map(Some);
        }
        if pending.restart {
            self.source = None;
        }
        self.checkpoint = pending.next;
        self.last = pending.counts;
        Ok(None)
    }
    /// State already reflects this local draft. Validate its parents, policy,
    /// and that it is the sole delivery since the state was current.
    pub fn mark_local(&mut self, ledger: &Ledger, draft: &Draft) -> Result<(), Error> {
        self.check(ledger)?;
        if self.pending.is_some() || self.checkpoint.is_partial() {
            return Err(Error::Stale);
        }
        if matches!(self.follow(), Follow::Signed) {
            return Err(Error::Protocol("cannot edit a signed-only view"));
        }
        if ledger.parents(&draft.hash).as_ref() != Some(&draft.parents)
            || ledger.author_of(&draft.hash) != Some(draft.author)
        {
            return Err(Error::Protocol("draft metadata does not match the ledger"));
        }
        let mut parents = draft.parents.clone();
        parents.sort();
        parents.dedup();
        let mut heads = self.heads().to_vec();
        heads.sort();
        heads.dedup();
        let since: Vec<_> = ledger
            .commits_since(self.checkpoint.cursor.delivery_heads())?
            .take(2)
            .collect();
        if parents != heads
            || since != [draft.hash]
            || self.checkpoint.cursor.view_point().write_permissions() != ledger.permissions()
        {
            return Err(Error::Stale);
        }
        if !ledger
            .ancestry(&[draft.hash])?
            .participates(ledger.history(), &draft.hash)?
            || is_rejected(&self.checkpoint, ledger, &draft.hash)
        {
            return Err(Error::Protocol("local draft does not participate"));
        }
        if matches!(self.follow(), Follow::Pinned(_)) {
            self.checkpoint.follow = Follow::Pinned(vec![draft.hash]);
        }
        self.checkpoint.cursor = MaterializationCursor::from_parts(
            ledger.heads(Heads::Delivered),
            ledger.view(self.follow()),
        );
        self.source = None;
        self.dirty = true;
        Ok(())
    }
    /// In-memory convenience: apply one planned source batch and all its
    /// follow-ups, then checkpoint the state. On failure the planner is
    /// poisoned; restore state and checkpoint together before continuing.
    /// [`DriveError::Apply`] and [`DriveError::Checkpoint`] preserve the
    /// materializer's error value and distinguish the failing phase.
    pub fn step<C: Codec, M: Materializer>(
        &mut self,
        document: &Document<C>,
        state: &mut M,
        budget: usize,
    ) -> Result<Step, DriveError<M::Error>> {
        self.check(document.ledger())?;
        if budget == 0 {
            return Err(Error::Limit("materialization budget must be positive").into());
        }
        if self.pending.is_some() {
            return Err(
                Error::Protocol("complete the outstanding batch before driving state").into(),
            );
        }
        let saved = self.checkpoint.clone();
        self.last = Counts::default();
        let result: Result<Step, DriveError<M::Error>> = (|| {
            let mut batch = self.plan(document.ledger(), budget)?;
            let did_work = batch.is_some();
            while let Some(current) = batch {
                let events = current
                    .events
                    .iter()
                    .cloned()
                    .map(|e| e.try_map_payload(|id| document.payload_by_id(id)))
                    .collect::<Result<Vec<_>, Error>>()?;
                let applied = state
                    .apply(document.history(), events)
                    .map_err(DriveError::Apply)?;
                batch = self.complete(document.ledger(), current, applied)?;
            }
            if did_work || self.dirty {
                state
                    .checkpoint(&self.checkpoint)
                    .map_err(DriveError::Checkpoint)?;
                self.dirty = false;
            }
            Ok(if self.is_current(document.ledger()) {
                Step::Current
            } else {
                Step::More
            })
        })();
        if result.is_err() {
            self.checkpoint = saved;
            self.poisoned = true;
        }
        result
    }
    pub fn update<C: Codec, M: Materializer>(
        &mut self,
        document: &Document<C>,
        state: &mut M,
    ) -> Result<Progress, DriveError<M::Error>> {
        let mut progress = Progress {
            heads_before: self.heads().to_vec(),
            ..Progress::default()
        };
        loop {
            let step = self.step(document, state, 1024)?;
            progress.delivered += self.last.delivered;
            progress.activated += self.last.activated;
            progress.retracted += self.last.retracted;
            progress.rejected += self.last.rejected;
            progress.rebuilt |= self.last.rebuilt;
            if step == Step::Current {
                break;
            }
        }
        progress.heads_after = self.heads().to_vec();
        Ok(progress)
    }
    /// Edit locally without echoing the resulting commit to this state.
    /// Closure errors are returned unchanged as [`DriveError::Edit`]. An edit
    /// failure may have mutated state, so it poisons the driver. Preflight
    /// errors do not run the closure or poison untouched state.
    pub fn change<C: Codec, M: Materializer, E: std::fmt::Display>(
        &mut self,
        document: &mut Document<C>,
        state: &mut M,
        edit: impl FnOnce(&mut M) -> Result<Option<Vec<u8>>, E>,
    ) -> Result<Option<Draft>, DriveError<M::Error, E>> {
        self.check(document.ledger())?;
        if !self.is_current(document.ledger()) {
            return Err(Error::Stale.into());
        }
        if matches!(self.follow(), Follow::Signed) {
            return Err(Error::Protocol("cannot edit a signed-only view").into());
        }
        if !matches!(
            document.permissions().get(&document.author()),
            Some(WritePermission::Unbounded)
        ) {
            return Err(Error::Protocol(
                "local author requires unbounded write permission to edit",
            )
            .into());
        }
        let bytes = match edit(state) {
            Ok(None) => return Ok(None),
            Ok(Some(bytes)) => bytes,
            Err(error) => {
                self.poisoned = true;
                return Err(DriveError::Edit(error));
            }
        };
        let result = document
            .commit(bytes, Some(self.heads().to_vec()))
            .and_then(|draft| {
                self.mark_local(document.ledger(), &draft)?;
                Ok(Some(draft))
            });
        if result.is_err() {
            self.poisoned = true;
        }
        result.map_err(DriveError::Stockfoil)
    }
}
fn is_rejected(checkpoint: &Checkpoint, ledger: &Ledger, hash: &CommitHash) -> bool {
    checkpoint.rejected.contains_key(hash)
        || checkpoint
            .rejected
            .iter()
            .any(|(root, cascade)| *cascade && ledger.is_ancestor(root, hash))
}

/// An immutable batch of events and its completion token.
///
/// Inspect [`events`](Self::events) or [`resolve`](Self::resolve) payload IDs
/// without consuming the token, then pass it to [`Materialization::complete`].
/// Clones identify the same work; completing one invalidates all of them.
///
/// ```compile_fail
/// # use stockfoil::Batch;
/// fn alter_events(batch: &mut Batch) {
///     batch.events.clear(); // events cannot be changed while retaining the token
/// }
/// ```
///
/// ```compile_fail
/// # use stockfoil::Batch;
/// fn alter_rebuild(batch: &mut Batch) {
///     batch.rebuild = false;
/// }
/// ```
#[derive(Debug, Clone)]
pub struct Batch {
    events: Vec<Event<PayloadId>>,
    rebuild: bool,
    planner: u64,
    ticket: u64,
}
impl Batch {
    /// Planned events, in application order. The completion token always
    /// refers to exactly these events.
    pub fn events(&self) -> &[Event<PayloadId>] {
        &self.events
    }

    /// Whether this batch begins a rebuild with [`Event::Reset`].
    pub fn is_rebuild(&self) -> bool {
        self.rebuild
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
    pub fn resolve<P>(&self, mut f: impl FnMut(PayloadId) -> P) -> Vec<Event<P>> {
        self.events
            .iter()
            .cloned()
            .map(|e| e.map_payload(&mut f))
            .collect()
    }
}
/// An incremental state-maintenance event emitted by a [`Materialization`].
///
/// `P` is the payload representation: [`Bytes`] when driving an in-memory
/// [`Materializer`], or [`PayloadId`] when planning [`Batch`]es against a
/// [`Ledger`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event<P> {
    /// Clear all materialized state in preparation for a clean replay of
    /// history.
    ///
    /// Emitted when `supports_visibility_changes` is `false` and a permission
    /// change, branch switch, or cascading payload rejection retracts an
    /// already-active commit.
    Reset,
    /// First delivery of a commit to this materialization.
    ///
    /// Every commit in the graph is delivered once, even when `active` is
    /// `false` (for example, when authored by a revoked writer or outside a
    /// pinned branch), because later active commits may reference its
    /// structural metadata. Record any causal metadata your CRDT needs, and
    /// apply user-visible effects only when `active` is `true`.
    Deliver {
        hash: CommitHash,
        author: Author,
        parents: Vec<CommitHash>,
        payload: P,
        active: bool,
    },
    /// A previously delivered commit has entered the active view (for example,
    /// after a permission grant or branch change). Only emitted when
    /// `supports_visibility_changes` is `true`.
    Activate {
        hash: CommitHash,
    },
    /// A previously active commit has left the active view (for example, after
    /// a write-frontier revocation, branch change, or cascading payload
    /// rejection). Only emitted when `supports_visibility_changes` is `true`.
    Retract {
        hash: CommitHash,
    },
    /// Delivers the payload of an older commit that the application requested
    /// via [`Applied::wanted`] in the preceding batch round.
    Payload {
        hash: CommitHash,
        payload: P,
    },
}
impl<P> Event<P> {
    pub fn map_payload<Q>(self, f: impl FnOnce(P) -> Q) -> Event<Q> {
        match self {
            Self::Reset => Event::Reset,
            Self::Deliver {
                hash,
                author,
                parents,
                payload,
                active,
            } => Event::Deliver {
                hash,
                author,
                parents,
                payload: f(payload),
                active,
            },
            Self::Activate { hash } => Event::Activate { hash },
            Self::Retract { hash } => Event::Retract { hash },
            Self::Payload { hash, payload } => Event::Payload {
                hash,
                payload: f(payload),
            },
        }
    }
    pub fn try_map_payload<Q, E>(self, f: impl FnOnce(P) -> Result<Q, E>) -> Result<Event<Q>, E> {
        Ok(match self {
            Self::Reset => Event::Reset,
            Self::Deliver {
                hash,
                author,
                parents,
                payload,
                active,
            } => Event::Deliver {
                hash,
                author,
                parents,
                payload: f(payload)?,
                active,
            },
            Self::Activate { hash } => Event::Activate { hash },
            Self::Retract { hash } => Event::Retract { hash },
            Self::Payload { hash, payload } => Event::Payload {
                hash,
                payload: f(payload)?,
            },
        })
    }
}
/// Feedback returned by the application after applying a batch of [`Event`]s.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Newly delivered commits from this batch whose payloads are semantically
    /// invalid for the application.
    pub rejected: Vec<Rejection>,
    /// Previously delivered commits whose payloads the application needs
    /// delivered in an immediate follow-up [`Event::Payload`] batch (for
    /// example, when retracting a winning write uncovers an older overwritten
    /// commit whose payload was not kept in memory).
    pub wanted: Vec<CommitHash>,
}
impl Applied {
    /// Mark a newly delivered commit from the current batch as permanently
    /// rejected (without tainting its descendants).
    pub fn reject(mut self, hash: CommitHash, reason: impl Into<String>) -> Self {
        self.rejected.push(Rejection {
            hash,
            reason: reason.into(),
            cascade: false,
        });
        self
    }
    /// Request that `hash`'s payload be delivered via [`Event::Payload`] in a
    /// follow-up batch before the current step finishes.
    pub fn want(mut self, hash: CommitHash) -> Self {
        self.wanted.push(hash);
        self
    }
}
/// Permanent per-materialization rejection of a commit with an invalid
/// application payload.
///
/// Rejecting a commit excludes its effects from this materialization (and is
/// recorded in its [`Checkpoint`]) without removing the commit from [`History`].
/// If `cascade` is `true`, any causal descendant of `hash` is also treated as
/// rejected. `reason` is diagnostic only and is not persisted in checkpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejection {
    pub hash: CommitHash,
    pub reason: String,
    pub cascade: bool,
}
/// In-memory application state driven by [`Replica`](crate::Replica) or
/// [`Materialization::update`] / [`Materialization::step`].
///
/// Applications with out-of-core or async storage consume [`Batch`]es from
/// [`Materialization::plan`] directly instead of implementing this trait.
pub trait Materializer {
    /// Application error type, returned unchanged inside [`DriveError::Apply`]
    /// or [`DriveError::Checkpoint`].
    type Error: std::fmt::Display;
    /// Apply a sequence of materialization events to the state.
    fn apply(
        &mut self,
        history: &History,
        events: Vec<Event<Bytes>>,
    ) -> Result<Applied, Self::Error>;
    /// Optional hook called with the updated [`Checkpoint`] after a step and
    /// all of its follow-up batches complete.
    fn checkpoint(&mut self, _: &Checkpoint) -> Result<(), Self::Error> {
        Ok(())
    }
    /// Whether this state can incrementally handle [`Event::Activate`] and
    /// [`Event::Retract`] when a previously delivered commit's visibility
    /// changes.
    ///
    /// Defaults to `false`, which causes [`Materialization`] to emit
    /// [`Event::Reset`] and replay active history whenever an already-applied
    /// commit needs to be retracted.
    fn supports_visibility_changes(&self) -> bool {
        false
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    More,
    Current,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    pub heads_before: Vec<CommitHash>,
    pub heads_after: Vec<CommitHash>,
    pub delivered: usize,
    pub activated: usize,
    pub retracted: usize,
    pub rejected: usize,
    pub rebuilt: bool,
}
impl Progress {
    pub fn changed(&self) -> bool {
        self.delivered > 0
            || self.activated > 0
            || self.retracted > 0
            || self.rejected > 0
            || self.rebuilt
    }
}
