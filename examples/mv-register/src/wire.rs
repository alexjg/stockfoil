//! Example-only, versioned BCS envelope. This is NOT a stable Stockfoil format.
//! Subduction authenticates the transport metadata; Stockfoil independently
//! authenticates the original authors and validates canonical fragment placement.
use std::collections::{BTreeSet, HashMap};

use eyre::{Result, ensure, eyre};
use serde::{Deserialize, Serialize};
use stockfoil::{
    Author, BlobHash, CommitContents, CommitHash, CommitMeta, FragmentRange, Signature,
    SignatureEvidence, UnverifiedItem, VerifiedItem,
};

use crate::{Assignment, MvRegisterMap};

pub(crate) const MAX_BLOB_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Envelope {
    version: u8,
    item: Item,
}

#[derive(Serialize, Deserialize)]
enum Item {
    Commit {
        commit: Commit,
        signatures: Vec<Evidence>,
    },
    Fragment {
        head: [u8; 32],
        boundary: Vec<[u8; 32]>,
        commits: Vec<Commit>,
        signatures: Vec<Evidence>,
    },
}

#[derive(Serialize, Deserialize)]
struct Commit {
    author: [u8; 32],
    // Preserve order and multiplicity: these are part of the signed hash.
    parents: Vec<[u8; 32]>,
    payload: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct Evidence {
    author: [u8; 32],
    commit: [u8; 32],
    signature: Vec<u8>,
}

pub(crate) struct Decoded {
    pub item: VerifiedItem,
    pub payloads: HashMap<CommitHash, Assignment>,
}

pub(crate) fn encode(doc: &MvRegisterMap, item: &UnverifiedItem) -> Result<Vec<u8>> {
    let commit = |meta: &CommitMeta| -> Result<Commit> {
        let (_, assignment) = doc
            .graph
            .publishable_commit(meta.hash())
            .ok_or_else(|| eyre!("cannot export an unadmitted payload"))?;
        let payload = bcs::to_bytes(assignment)?;
        ensure!(
            BlobHash::of_contents(&payload) == *meta.payload_hash(),
            "payload hash mismatch"
        );
        Ok(Commit {
            author: *meta.author().as_array(),
            parents: meta.parents().iter().map(|hash| *hash.as_bytes()).collect(),
            payload,
        })
    };
    let evidence = |signatures: &[SignatureEvidence]| {
        signatures
            .iter()
            .map(|s| Evidence {
                author: *s.author.as_array(),
                commit: *s.commit.as_bytes(),
                signature: s.signature.as_slice().to_vec(),
            })
            .collect()
    };
    let item = match item {
        UnverifiedItem::Commit { meta, signatures } => Item::Commit {
            commit: commit(meta)?,
            signatures: evidence(signatures),
        },
        UnverifiedItem::Fragment {
            range,
            commits,
            signatures,
            // Reconciliation hints are not part of this example's wire format.
            ..
        } => Item::Fragment {
            head: *range.head.as_bytes(),
            boundary: range.boundary.iter().map(|hash| *hash.as_bytes()).collect(),
            commits: commits.iter().map(commit).collect::<Result<_>>()?,
            signatures: evidence(signatures),
        },
    };
    let bytes = bcs::to_bytes(&Envelope { version: 1, item })?;
    ensure!(
        bytes.len() <= MAX_BLOB_BYTES,
        "example blob exceeds size limit"
    );
    Ok(bytes)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Decoded> {
    ensure!(
        bytes.len() <= MAX_BLOB_BYTES,
        "example blob exceeds size limit"
    );
    let envelope: Envelope = bcs::from_bytes(bytes)?;
    ensure!(
        envelope.version == 1,
        "unsupported example envelope version"
    );
    let mut raw_payloads = HashMap::new();
    let mut commit = |wire: Commit| -> Result<CommitMeta> {
        let meta = CommitMeta::new(CommitContents {
            author: Author::try_from(wire.author.as_slice())?,
            parents: wire.parents.into_iter().map(CommitHash::from).collect(),
            payload: BlobHash::of_contents(&wire.payload),
        });
        ensure!(
            raw_payloads.insert(*meta.hash(), wire.payload).is_none(),
            "duplicate member"
        );
        Ok(meta)
    };
    let evidence = |signatures: Vec<Evidence>| -> Result<Vec<SignatureEvidence>> {
        signatures
            .into_iter()
            .map(|s| {
                Ok(SignatureEvidence {
                    author: Author::try_from(s.author.as_slice())?,
                    commit: CommitHash::from(s.commit),
                    signature: Signature::from(
                        <[u8; 64]>::try_from(s.signature)
                            .map_err(|_| eyre!("signature must contain 64 bytes"))?,
                    ),
                })
            })
            .collect()
    };
    let item = match envelope.item {
        Item::Commit {
            commit: wire,
            signatures,
        } => UnverifiedItem::Commit {
            meta: commit(wire)?,
            signatures: evidence(signatures)?,
        },
        Item::Fragment {
            head,
            boundary,
            commits,
            signatures,
        } => {
            let count = boundary.len();
            let boundary: BTreeSet<_> = boundary.into_iter().map(CommitHash::from).collect();
            ensure!(boundary.len() == count, "duplicate boundary");
            UnverifiedItem::Fragment {
                // This example's BCS envelope omits optional reconciliation hints.
                checkpoints: BTreeSet::new(),
                range: FragmentRange {
                    head: CommitHash::from(head),
                    boundary,
                },
                commits: commits.into_iter().map(commit).collect::<Result<_>>()?,
                signatures: evidence(signatures)?,
            }
        }
    }
    .verify()?;
    // Signatures authenticate the hashes computed from the exact payload bytes,
    // not re-encoded decoded values. Interpret assignments only after verification.
    let payloads = raw_payloads
        .into_iter()
        .map(|(hash, bytes)| Ok((hash, bcs::from_bytes(&bytes)?)))
        .collect::<Result<_>>()?;
    Ok(Decoded { item, payloads })
}

#[cfg(test)]
mod tests {
    use super::*;
    use stockfoil::ItemId;

    #[test]
    fn envelopes_bind_payloads_signatures_and_version() {
        let mut doc = MvRegisterMap::new();
        let hash = doc.set("title", "hello");
        // A signed head is exportable even if it happens to be a checkpoint.
        let item = doc.graph.export(&ItemId::Commit(hash)).unwrap();
        let bytes = encode(&doc, &item).unwrap();
        assert_eq!(decode(&bytes).unwrap().item.id(), &ItemId::Commit(hash));
        for len in 0..bytes.len() {
            assert!(decode(&bytes[..len]).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode(&trailing).is_err());
        let mut envelope: Envelope = bcs::from_bytes(&bytes).unwrap();
        envelope.version = 2;
        assert!(decode(&bcs::to_bytes(&envelope).unwrap()).is_err());
        envelope.version = 1;
        let Item::Commit { commit, .. } = &mut envelope.item else {
            unreachable!()
        };
        commit.payload[0] ^= 1;
        assert!(decode(&bcs::to_bytes(&envelope).unwrap()).is_err());
        let mut envelope: Envelope = bcs::from_bytes(&bytes).unwrap();
        let Item::Commit { signatures, .. } = &mut envelope.item else {
            unreachable!()
        };
        signatures[0].signature[0] ^= 1;
        assert!(decode(&bcs::to_bytes(&envelope).unwrap()).is_err());
    }
}
