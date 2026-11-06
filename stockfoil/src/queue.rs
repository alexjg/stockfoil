//! Dependency-indexed, bounded waiting queue for verified [`Admission`]s.
//!
//! Operations are indexed both by dependency and by original parking order.
//! Rechecking an operation and moving it to another dependency does not make
//! it younger. Both indexes contain only live entries: a permanently missing
//! dependency cannot make completed entries accumulate as FIFO tombstones.
use crate::{Admission, CommitHash, Error, History, ItemId, Next, Receipt, Request, Ticket};
use std::collections::{BTreeMap, BTreeSet};

/// What a queued admission needs next once its awaited dependency is admitted.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum QueueNext<T> {
    /// Undelivered members need payload IDs. Reply to `op` with
    /// [`Reply::Bound`](crate::Reply::Bound) for `ticket`, then call
    /// [`History::advance`] to complete publication.
    Bind {
        context: T,
        op: Admission,
        ticket: Ticket,
        member_count: usize,
    },
    /// Every member was already delivered, so publication completed immediately.
    Done { context: T, receipt: Receipt },
    /// The queued item failed placement once its dependencies arrived.
    Failed { context: T, error: Error },
}

/// An item dropped to respect the parked-item cap.
///
/// Its admission has been dropped, but the caller gets its context back to
/// release staged payloads or transport bookkeeping. The item can be received
/// again later; eviction neither publishes nor rejects its commits.
#[derive(Debug)]
pub struct Evicted<T> {
    pub id: ItemId,
    pub context: T,
}

#[derive(Debug)]
struct Entry<T> {
    dependency: CommitHash,
    context: T,
    op: Admission,
}

/// Verified [`Admission`]s waiting for missing dependencies to be admitted.
///
/// Default cap: 1024 items. Each admission's metadata is bounded separately by
/// [`Limits`](crate::Limits); caller-owned context must have its own bounds.
/// At capacity, [`push`](Self::push) evicts the oldest parked operation and
/// returns its context. A zero cap immediately returns the incoming item.
#[derive(Debug)]
pub struct Queue<T = ()> {
    waiting: BTreeMap<CommitHash, BTreeSet<u64>>,
    entries: BTreeMap<u64, Entry<T>>,
    capacity: usize,
    next: u64,
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Queue<T> {
    pub fn new() -> Self {
        Self::with_capacity(1024)
    }

    /// Maximum number of parked admissions, not bytes or reserved allocation.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            waiting: BTreeMap::new(),
            entries: BTreeMap::new(),
            capacity,
            next: 0,
        }
    }

    pub(crate) fn missing(&self) -> impl Iterator<Item = CommitHash> + '_ {
        self.waiting.keys().copied()
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of operations currently waiting in the queue.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Park an admission currently waiting for `waiting_on`. If the cap is
    /// exceeded, return the oldest item and its context for caller cleanup.
    /// With a zero cap the returned item is the incoming one.
    ///
    /// Passing an admission that is not verified and waiting on this dependency
    /// is a protocol error. An exhausted age counter fails before any existing
    /// entry is evicted.
    pub fn push(
        &mut self,
        waiting_on: CommitHash,
        context: T,
        op: Admission,
    ) -> Result<Option<Evicted<T>>, Error> {
        if op.waiting_on() != Some(waiting_on) {
            return Err(Error::Protocol(
                "queue expects a verified waiting admission",
            ));
        }
        let entry = Entry {
            dependency: waiting_on,
            context,
            op,
        };
        if self.capacity == 0 {
            return Ok(Some(Self::evicted(entry)));
        }
        let age = self.next;
        let next = age.checked_add(1).ok_or(Error::Exhausted("queue ages"))?;
        let evicted = if self.len() == self.capacity {
            let oldest = *self.entries.first_key_value().unwrap().0;
            Some(Self::evicted(self.remove(oldest)))
        } else {
            None
        };
        self.next = next;
        self.insert(age, entry);
        Ok(evicted)
    }

    fn insert(&mut self, age: u64, entry: Entry<T>) {
        self.waiting
            .entry(entry.dependency)
            .or_default()
            .insert(age);
        self.entries.insert(age, entry);
    }

    fn remove(&mut self, age: u64) -> Entry<T> {
        let entry = self.entries.remove(&age).unwrap();
        let waiting = self.waiting.get_mut(&entry.dependency).unwrap();
        waiting.remove(&age);
        if waiting.is_empty() {
            self.waiting.remove(&entry.dependency);
        }
        entry
    }

    fn evicted(entry: Entry<T>) -> Evicted<T> {
        Evicted {
            id: entry.op.meta().expect("verified header").id.clone(),
            context: entry.context,
        }
    }

    /// Advance the next queued operation whose awaited dependency is now
    /// admitted in `history`.
    ///
    /// If another dependency is still missing, re-index the operation without
    /// changing its original parking age, then continue searching. Publication
    /// by a returned `Bind` operation can release further entries on later polls.
    pub fn poll(&mut self, history: &mut History) -> Option<QueueNext<T>> {
        loop {
            let dep = *self.waiting.keys().find(|hash| history.is_admitted(hash))?;
            let age = *self.waiting[&dep].first().unwrap();
            let Entry {
                context, mut op, ..
            } = self.remove(age);
            match history.advance(&mut op) {
                Next::Waiting(other_dep) => {
                    self.insert(
                        age,
                        Entry {
                            dependency: other_dep,
                            context,
                            op,
                        },
                    );
                }
                Next::Done(receipt) => {
                    return Some(QueueNext::Done {
                        context,
                        receipt: receipt.clone(),
                    });
                }
                Next::Failed(error) => {
                    return Some(QueueNext::Failed {
                        context,
                        error: error.clone(),
                    });
                }
                Next::Request(ticket, Request::Bind { meta }) => {
                    let member_count = meta.len();
                    return Some(QueueNext::Bind {
                        context,
                        op,
                        ticket,
                        member_count,
                    });
                }
                other => unreachable!("unexpected state from queued admission: {other:?}"),
            }
        }
    }
}

#[cfg(test)]
mod tests;
