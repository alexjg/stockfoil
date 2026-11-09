//! Publish prepared local metadata without assuming hashing or authentication.
#[cfg(verus_keep_ghost)]
use super::clock::NodeIdx;
use super::{
    api_error::Error, author::Author, commit_hash::CommitHash, draft::Draft, history::History,
    history_data::PayloadId, ledger::Ledger, topology::InsertError,
};
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {

/// A local commit's identity, ready to sign and publish.
/// Publication checks availability of parents; it does not authenticate the hash.
#[cfg_attr(not(verus_keep_ghost), derive(Debug, Clone, PartialEq, Eq))]
pub struct Prepared {
    pub hash: CommitHash,
    pub(crate) author: Author,
    pub(crate) parents: Vec<CommitHash>,
}

/// One commit's delivery outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivered {
    pub hash: CommitHash,
    /// The retained payload ID. For a duplicate this is the earlier delivery's ID.
    pub payload: PayloadId,
    /// Whether this call delivered the commit to the history for the first time.
    pub newly_delivered: bool,
}

pub(crate) open spec fn depth_value(hash:CommitHash) -> nat {
    if exists|i:int| 0 <= i < 32 && hash.0@[i] != 0 {
        choose|i:nat| i < 32 && hash.0@[i as int] != 0 && forall|j:int| 0 <= j < i ==> hash.0@[j] == 0
    } else { 32 }
}

/// Sedimentree depth is exactly the number of leading zero bytes.
pub(crate) fn depth(hash:&CommitHash) -> (out:usize)
    ensures out == depth_value(*hash), out <= 32,
        forall|i:int| 0 <= i < out ==> hash.0@[i] == 0,
        out < 32 ==> hash.0@[out as int] != 0,
{
    let mut i = 0usize;
    while i < 32
        invariant i <= 32, forall|j:int| 0 <= j < i ==> hash.0@[j] == 0,
        decreases 32 - i,
    {
        if hash.0[i] != 0 {
            assert(exists|k:int| 0 <= k < 32 && hash.0@[k] != 0);
            assert(depth_value(*hash) == i);
            return i;
        }
        i += 1;
    }
    i
}

impl History {
    pub closed spec fn epoch_view(&self) -> u64 { self.epoch }
    /// Increments when history changes; dependency waiters can retry after it changes.
    pub fn epoch(&self) -> (out:u64) ensures out == self.epoch_view(), { self.epoch }

    pub(crate) open spec fn first_missing(&self,parents:Seq<CommitHash>,hash:CommitHash) -> bool {
        exists|i:int| 0 <= i < parents.len() && parents[i] == hash && !self.known(hash)
            && forall|j:int| 0 <= j < i ==> self.known(parents[j])
    }
    pub(crate) fn publication_parents(&self,parents:&[CommitHash]) -> (out:Result<(),Error>)
        ensures match out {
            Ok(()) => forall|i:int| 0 <= i < parents.len() ==> self.known(parents[i]),
            Err(e) => e matches Error::MissingParent(hash) && self.first_missing(parents@,hash),
        },
    {
        proof { self.representation(); }
        let mut i = 0;
        while i < parents.len()
            invariant i <= parents.len(), forall|j:int| 0 <= j < i ==> self.known(parents[j]),
            decreases parents.len() - i,
        {
            proof { self.representation(); }
            if !self.topology.by_hash().contains_key(&parents[i]) {
                assert(self.first_missing(parents@,parents[i as int]));
                return Err(Error::MissingParent(parents[i]));
            }
            i += 1;
        }
        Ok(())
    }
    pub(crate) open spec fn publication_frame(&self,before:&Self) -> bool {
        self.admitted_heads == before.admitted_heads && self.signed_drafts == before.signed_drafts
            && self.signatures == before.signatures && self.inventory == before.inventory
    }
    pub closed spec fn publish_outcome(&self,before:&Self,prepared:&Prepared,payload:PayloadId,out:Result<Delivered,Error>) -> bool {
        self.publish_details(before,prepared,payload,out)
    }
    pub(crate) open spec fn publish_details(&self,before:&Self,prepared:&Prepared,payload:PayloadId,out:Result<Delivered,Error>) -> bool {
        if before.known(prepared.hash) {
            *self == *before && (out matches Ok(delivered) && delivered.hash == prepared.hash && !delivered.newly_delivered
                && delivered.payload == before.topology.nodes_view()[before.read_state().hashes[prepared.hash]@ as int].payload)
        } else {
            match out {
                Err(e) => *self == *before && match e {
                    Error::MissingParent(h) => before.first_missing(prepared.parents@,h),
                    Error::Exhausted(reason) => (forall|i:int| 0 <= i < prepared.parents.len() ==> before.known(prepared.parents[i]))
                        && if before.epoch == u64::MAX { reason == "history epoch" }
                        else { reason == "topology capacity" && (before.topology.nodes_view().len() > u32::MAX || prepared.parents.len() > u32::MAX
                            || before.topology.ids_view().len() + prepared.parents.len() > usize::MAX || before.topology.tips_view().len() == u32::MAX) },
                    _ => false,
                },
                Ok(delivered) => delivered.hash == prepared.hash && delivered.payload == payload && delivered.newly_delivered
                    && before.epoch < u64::MAX && self.epoch == before.epoch + 1 && self.publication_frame(before)
                    && exists|idx:NodeIdx| self.topology.inserted(before.topology,idx,prepared.hash,prepared.author,prepared.parents@,payload,depth_value(prepared.hash) as u8)
                        && self.drafts@ == before.drafts@.insert(idx),
            }
        }
    }
    pub(crate) proof fn publish_facts(&self,before:&Self,prepared:&Prepared,payload:PayloadId,out:Result<Delivered,Error>)
        requires self.publish_outcome(before,prepared,payload,out),
        ensures self.publish_details(before,prepared,payload,out),
    {}

    pub(crate) proof fn published_metadata(&self,before:&Self,prepared:&Prepared,payload:PayloadId,delivered:Delivered)
        requires self.publish_outcome(before,prepared,payload,Ok(delivered)), delivered.newly_delivered,
        ensures self.known(prepared.hash), self.event_author(prepared.hash) == prepared.author,
            self.event_parents(prepared.hash) == prepared.parents@,
            self.read_state().extends(before.read_state()),
    {
        self.publish_facts(before,prepared,payload,Ok(delivered));
        let idx = choose|idx:NodeIdx| self.topology.inserted(before.topology,idx,prepared.hash,prepared.author,prepared.parents@,payload,depth_value(prepared.hash) as u8)
            && self.drafts@ == before.drafts@.insert(idx);
        self.representation(); before.representation(); self.topology.permission_facts(); self.topology.hash_facts(); before.topology.hash_facts();
        let node = self.topology.nodes_view()[idx@ as int];
        let parents = super::storage::parents(node,self.topology.ids_view());
        assert(parents =~= prepared.parents@.map(|i:int,h:CommitHash| before.read_state().hashes[h]));
        assert forall|i:int| 0 <= i < prepared.parents.len() implies self.event_parents(prepared.hash)[i] == prepared.parents[i] by {
            let p = prepared.parents[i];
            let old_idx = before.read_state().hashes[p];
            assert(old_idx@ < before.topology.nodes_view().len());
            assert(self.topology.nodes_view()[old_idx@ as int] == before.topology.nodes_view()[old_idx@ as int]);
        }
        assert(self.event_parents(prepared.hash) =~= prepared.parents@);
    }

    /// Deliver an unadmitted local commit (see [`CommitBuilder`](crate::CommitBuilder)).
    /// Duplicate hashes retain the original record and payload ID, even if the
    /// supplied metadata differs. Missing parents and exhausted counters return
    /// errors without publishing a node or changing the epoch.
    pub fn publish_commit(&mut self,prepared:Prepared,payload:PayloadId) -> (out:Result<Delivered,Error>)
        ensures final(self).publish_outcome(old(self),&prepared,payload,out),
            out is Err ==> *final(self) == *old(self),
            final(self).history_id() == old(self).history_id(),
    {
        let ghost before = *self;
        proof { self.representation(); self.topology.established(); }
        if let Some(idx) = self.topology.by_hash().get(&prepared.hash) {
            proof { self.topology.hash_facts(); }
            return Ok(Delivered { hash:prepared.hash,payload:self.topology.nodes()[idx.index()].payload,newly_delivered:false });
        }
        self.publication_parents(&prepared.parents)?;
        let epoch = match self.epoch.checked_add(1) { Some(epoch) => epoch, None => return Err(Error::Exhausted("history epoch")) };
        let level = depth(&prepared.hash);
        let idx = match self.topology.insert(prepared.hash,prepared.author,&prepared.parents,payload,level as u8) {
            Ok(idx) => idx,
            Err(InsertError::Capacity) => return Err(Error::Exhausted("topology capacity")),
            Err(InsertError::Missing(h)) => return Err(Error::MissingParent(h)),
            Err(InsertError::Duplicate) => return Err(Error::Protocol("commit already delivered")),
        };
        proof { super::stdlib::node_order_model(); }
        self.drafts.insert(idx);
        self.epoch = epoch;
        proof { self.representation(); }
        Ok(Delivered { hash:prepared.hash,payload,newly_delivered:true })
    }
}

impl<O,Q> Ledger<O,Q> {
    pub closed spec fn author_view(&self) -> Author { self.author }
    pub(crate) proof fn author_facts(&self) ensures self.author_view() == self.author, {}
    pub fn author(&self) -> (out:Author) ensures out == self.author_view(), { self.author }
    pub closed spec fn commit_outcome(&self,before:&Self,prepared:&Prepared,payload:PayloadId,out:Result<Draft,Error>) -> bool {
        self.commit_details(before,prepared,payload,out)
    }
    pub(crate) open spec fn commit_details(&self,before:&Self,prepared:&Prepared,payload:PayloadId,out:Result<Draft,Error>) -> bool {
        self.author == before.author && self.options == before.options && self.permissions == before.permissions
            && self.policy_epoch == before.policy_epoch && self.queue == before.queue
            && if prepared.author != before.author {
                *self == *before && out == Err(Error::Protocol("prepared commit has a different local author"))
            } else {
                exists|published:Result<Delivered,Error>| self.history.publish_outcome(&before.history,prepared,payload,published)
                    && match published {
                        Err(e) => out == Err(e),
                        Ok(_) => out matches Ok(draft) && draft.hash == prepared.hash && draft.author == prepared.author && draft.parents@ == prepared.parents@,
                    }
            }
    }
    pub(crate) proof fn commit_facts(&self,before:&Self,prepared:&Prepared,payload:PayloadId,out:Result<Draft,Error>)
        requires self.commit_outcome(before,prepared,payload,out),
        ensures self.commit_details(before,prepared,payload,out),
    {}
    /// Deliver a local draft. Unsigned drafts are not durable: persist a local
    /// journal if signing may be delayed across a restart. Duplicate hashes keep
    /// stored metadata; the returned Draft describes the supplied Prepared value.
    pub fn commit(&mut self,prepared:Prepared,payload:PayloadId) -> (out:Result<Draft,Error>)
        ensures final(self).commit_outcome(old(self),&prepared,payload,out),
            out is Err ==> *final(self) == *old(self),
    {
        let ghost before = *self;
        if !prepared.author.same_as(&self.author) {
            return Err(Error::Protocol("prepared commit has a different local author"));
        }
        let draft = Draft { hash:prepared.hash,author:prepared.author,parents:prepared.parents.clone() };
        let result = self.history.publish_commit(prepared,payload);
        proof { self.history.publish_facts(&before.history,&prepared,payload,result); }
        match result { Ok(_) => Ok(draft), Err(e) => Err(e) }
    }
}
} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared(id: u8, parents: &[u8]) -> Prepared {
        Prepared {
            hash: CommitHash([id; 32]),
            author: Author([7; 32]),
            parents: parents.iter().map(|id| CommitHash([*id; 32])).collect(),
        }
    }

    #[test]
    fn publication_exhaustion_preserves_history_and_precedence() {
        let mut history = History::new();
        history.epoch = u64::MAX - 1;
        assert!(
            history
                .publish_commit(prepared(1, &[]), 17)
                .unwrap()
                .newly_delivered
        );
        assert_eq!(history.epoch(), u64::MAX);
        let before = format!("{history:?}");
        assert!(matches!(
            history.publish_commit(prepared(2, &[1]), 18),
            Err(Error::Exhausted("history epoch"))
        ));
        assert_eq!(format!("{history:?}"), before);
        assert!(
            matches!(history.publish_commit(prepared(2, &[1, 3, 4]), 18), Err(Error::MissingParent(h)) if h == CommitHash([3;32]))
        );
        assert_eq!(format!("{history:?}"), before);
        let duplicate = history.publish_commit(prepared(1, &[99]), 100).unwrap();
        assert!(!duplicate.newly_delivered);
        assert_eq!(duplicate.payload, 17);
        assert_eq!(format!("{history:?}"), before);
    }

    pub(super) fn ledger() -> Ledger<(), ()> {
        Ledger {
            history: History::new(),
            author: Author([7; 32]),
            options: (),
            permissions: super::super::write_permissions::WritePermissions::default(),
            policy_epoch: 0,
            queue: (),
        }
    }

    #[test]
    fn ledger_keeps_ordered_metadata_and_first_payload() {
        let mut ledger = ledger();
        ledger.commit(prepared(1, &[]), 10).unwrap();
        ledger.commit(prepared(2, &[1, 1]), 20).unwrap();
        assert_eq!(
            ledger.parents(&CommitHash([2; 32])).unwrap(),
            vec![CommitHash([1; 32]); 2]
        );
        assert_eq!(
            ledger.author_of(&CommitHash([2; 32])),
            Some(Author([7; 32]))
        );
        let before = format!("{:?}", ledger.history);
        let submitted = ledger.commit(prepared(2, &[99]), 30).unwrap();
        assert_eq!(submitted.parents, vec![CommitHash([99; 32])]);
        assert_eq!(format!("{:?}", ledger.history), before);
        let result = ledger.history.publish_commit(prepared(2, &[]), 40).unwrap();
        assert_eq!(result.payload, 20);
    }

    #[test]
    fn ledger_rejects_wrong_author_before_duplicate_or_missing_parent() {
        let mut ledger = ledger();
        ledger.commit(prepared(1, &[]), 10).unwrap();
        let before = format!("{:?}", ledger.history);
        let mut wrong = prepared(1, &[99]);
        wrong.author = Author([8; 32]);
        assert!(matches!(
            ledger.commit(wrong, 20),
            Err(Error::Protocol(
                "prepared commit has a different local author"
            ))
        ));
        assert_eq!(format!("{:?}", ledger.history), before);
        assert!(
            matches!(ledger.commit(prepared(2, &[1, 3, 4]), 20), Err(Error::MissingParent(h)) if h == CommitHash([3;32]))
        );
        assert_eq!(format!("{:?}", ledger.history), before);
    }

    #[test]
    fn leading_zero_depth_covers_every_position_and_byte() {
        assert_eq!(depth(&CommitHash([0; 32])), 32);
        for position in 0..32 {
            for byte in 1..=255 {
                let mut hash = [0; 32];
                hash[position] = byte;
                assert_eq!(depth(&CommitHash(hash)), position);
            }
        }
    }
}
