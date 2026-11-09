//! The actual post-hashing payload registration and local publication path.
use super::{
    api_error::Error, document::Document, draft::Draft, history_data::PayloadId,
    publication::Prepared,
};
use std::sync::Arc;
#[cfg(not(verus_keep_ghost))]
use verus_builtin_macros::verus;
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

verus! {
impl<C,O,Q> Document<C,O,Q> {
    pub(crate) open spec fn stored_for_publication(&self,before:&Self,prepared:&Prepared,bytes:Arc<[u8]>,id:PayloadId) -> bool {
        if before.index@.contains_key(prepared.hash) {
            id == before.index@[prepared.hash] && self.index == before.index && self.payloads == before.payloads
        } else {
            before.payloads.len() <= u64::MAX && id == before.payloads.len()
                && self.payloads@ == before.payloads@.push(bytes) && self.index@ == before.index@.insert(prepared.hash,id)
        }
    }
    pub(crate) open spec fn publication_outcome(&self,before:&Self,prepared:&Prepared,bytes:Arc<[u8]>,out:Result<Draft,Error>) -> bool {
        self.codec == before.codec && self.staged == before.staged
            && if !before.index@.contains_key(prepared.hash) && before.payloads.len() > u64::MAX {
                *self == *before && out == Err(Error::Exhausted("payload IDs"))
            } else {
                exists|id:PayloadId| self.stored_for_publication(before,prepared,bytes,id)
                    && self.ledger.commit_outcome(&before.ledger,prepared,id,out)
            }
    }
    /// Register bytes before publishing, preserving the first slot for each hash.
    /// A later Ledger error retains that registration, as in the public commit path.
    pub(crate) fn publish_prepared(&mut self,prepared:Prepared,bytes:Arc<[u8]>) -> (out:Result<Draft,Error>)
        ensures final(self).publication_outcome(old(self),&prepared,bytes,out),
    {
        let ghost before = *self;
        proof { super::stdlib::commit_key_model(); }
        let id = match self.index.get(&prepared.hash) {
            Some(id) => *id,
            None => {
                if self.payloads.len() as u128 > u64::MAX as u128 { return Err(Error::Exhausted("payload IDs")); }
                let id = self.payloads.len() as PayloadId;
                self.payloads.push(bytes);
                self.index.insert(prepared.hash,id);
                id
            },
        };
        let result = self.ledger.commit(prepared,id);
        assert(self.stored_for_publication(&before,&prepared,bytes,id));
        result
    }
}
} // verus!

#[cfg(test)]
mod tests {
    use super::super::{
        author::Author, commit_hash::CommitHash, history::History, ledger::Ledger,
        write_permissions::WritePermissions,
    };
    use super::*;
    use std::collections::HashMap;

    fn document() -> Document<(), (), ()> {
        Document {
            ledger: Ledger {
                history: History::new(),
                author: Author([7; 32]),
                options: (),
                permissions: WritePermissions::default(),
                policy_epoch: 0,
                queue: (),
            },
            codec: (),
            payloads: Vec::new(),
            index: HashMap::new(),
            staged: HashMap::new(),
        }
    }
    fn prepared(id: u8, parents: Vec<CommitHash>) -> Prepared {
        Prepared {
            hash: CommitHash([id; 32]),
            author: Author([7; 32]),
            parents,
        }
    }

    #[test]
    fn publication_keeps_first_bytes_and_slot() {
        let mut document = document();
        let first: Arc<[u8]> = Arc::from(&b"first"[..]);
        document
            .publish_prepared(prepared(1, vec![]), first.clone())
            .unwrap();
        let draft = document
            .publish_prepared(
                prepared(1, vec![CommitHash([99; 32])]),
                Arc::from(&b"second"[..]),
            )
            .unwrap();
        assert_eq!(draft.parents, vec![CommitHash([99; 32])]);
        assert_eq!(document.payloads.len(), 1);
        assert!(Arc::ptr_eq(&document.payloads[0], &first));
        assert_eq!(document.index[&CommitHash([1; 32])], 0);
        assert_eq!(document.ledger.history.epoch(), 1);
    }

    #[test]
    fn failed_publication_retains_registered_bytes_for_retry() {
        let mut document = document();
        let first: Arc<[u8]> = Arc::from(&b"child"[..]);
        let before = format!("{:?}", document.ledger.history);
        let parents = vec![CommitHash([1; 32])];
        assert!(matches!(
            document.publish_prepared(prepared(2, parents.clone()), first.clone()),
            Err(Error::MissingParent(_))
        ));
        assert_eq!(format!("{:?}", document.ledger.history), before);
        assert_eq!(document.index[&CommitHash([2; 32])], 0);
        assert!(Arc::ptr_eq(&document.payloads[0], &first));
        document
            .publish_prepared(prepared(1, vec![]), Arc::from(&b"root"[..]))
            .unwrap();
        document
            .publish_prepared(prepared(2, parents), Arc::from(&b"replacement"[..]))
            .unwrap();
        assert_eq!(document.payloads.len(), 2);
        assert!(Arc::ptr_eq(&document.payloads[0], &first));
        assert_eq!(
            document
                .ledger
                .history
                .publish_commit(prepared(2, vec![]), 99)
                .unwrap()
                .payload,
            0
        );
    }
}
