//! A real, deliberately unauthenticated transport codec for the contract fixtures.
//! Decoding checks wire structure, not payload commitments, signatures, or graph rules.

use std::{collections::BTreeMap, io};

use serde::{Deserialize, Serialize};
use stockfoil::{
    Author, BlobHash, CommitContents, CommitMeta, CommitSignature, DecodedCommit, DecodedFragment,
    DecodedLooseCommit, FragmentRef, Signature, SignedCommitRef, SyncCodec,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Write {
    pub key: String,
    pub value: String,
    pub nonce: u64,
}

#[derive(Clone, Debug)]
pub struct Record {
    pub meta: CommitMeta,
    pub payload: Write,
}

#[derive(Default)]
pub struct BatchCodec {
    pub loose_encodes: usize,
    pub fragment_encodes: usize,
}

#[derive(Serialize, Deserialize)]
struct LooseWire {
    author: [u8; 32],
    parents: Vec<[u8; 32]>,
    payload_hash: [u8; 32],
    payload: Write,
    signature: Vec<u8>,
}

/// Every column has one entry per commit. Authors and strings are interned across
/// the whole batch (including proof authors); parents retain their original order.
#[derive(Default, Serialize, Deserialize)]
struct BatchWire {
    authors: Vec<[u8; 32]>,
    strings: Vec<String>,
    commit_authors: Vec<u32>,
    parents: Vec<Vec<[u8; 32]>>,
    payload_hashes: Vec<[u8; 32]>,
    keys: Vec<u32>,
    values: Vec<u32>,
    nonces: Vec<u64>,
    proofs: Vec<ProofWire>,
}

#[derive(Serialize, Deserialize)]
struct ProofWire {
    author: u32,
    commit: [u8; 32],
    signature: Vec<u8>,
}

fn invalid_data(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

fn signature(bytes: Vec<u8>) -> io::Result<Signature> {
    let bytes: [u8; 64] = bytes
        .try_into()
        .map_err(|_| invalid_data("signature must contain exactly 64 bytes"))?;
    Ok(Signature::from(bytes))
}

fn table_entry<T>(table: &[T], index: u32) -> io::Result<&T> {
    let index = usize::try_from(index).map_err(invalid_data)?;
    table
        .get(index)
        .ok_or_else(|| invalid_data("table index out of bounds"))
}

fn intern<T: Clone + Ord>(
    value: T,
    table: &mut Vec<T>,
    indices: &mut BTreeMap<T, u32>,
) -> io::Result<u32> {
    if let Some(index) = indices.get(&value) {
        return Ok(*index);
    }
    let index = u32::try_from(table.len()).map_err(invalid_data)?;
    table.push(value.clone());
    indices.insert(value, index);
    Ok(index)
}

fn loose_bytes(record: &Record, signature: Signature) -> io::Result<Vec<u8>> {
    bcs::to_bytes(&LooseWire {
        author: *record.meta.author().as_array(),
        parents: record
            .meta
            .parents()
            .iter()
            .map(|hash| *hash.as_bytes())
            .collect(),
        payload_hash: (*record.meta.payload_hash()).into(),
        payload: record.payload.clone(),
        signature: signature.as_slice().to_vec(),
    })
    .map_err(invalid_data)
}

fn batch_bytes(records: &[Record], proofs: &[CommitSignature]) -> io::Result<Vec<u8>> {
    let mut wire = BatchWire::default();
    let mut authors = BTreeMap::new();
    let mut strings = BTreeMap::new();
    for record in records {
        wire.commit_authors.push(intern(
            *record.meta.author().as_array(),
            &mut wire.authors,
            &mut authors,
        )?);
        wire.parents.push(
            record
                .meta
                .parents()
                .iter()
                .map(|hash| *hash.as_bytes())
                .collect(),
        );
        wire.payload_hashes
            .push((*record.meta.payload_hash()).into());
        wire.keys.push(intern(
            record.payload.key.clone(),
            &mut wire.strings,
            &mut strings,
        )?);
        wire.values.push(intern(
            record.payload.value.clone(),
            &mut wire.strings,
            &mut strings,
        )?);
        wire.nonces.push(record.payload.nonce);
    }
    for proof in proofs {
        wire.proofs.push(ProofWire {
            author: intern(*proof.author.as_array(), &mut wire.authors, &mut authors)?,
            commit: *proof.commit.as_bytes(),
            signature: proof.signature.as_slice().to_vec(),
        });
    }
    bcs::to_bytes(&wire).map_err(invalid_data)
}

/// Encode arbitrary records and detached proofs without authenticating them.
/// Inconsistent payload commitments and unrelated/duplicate proofs are preserved
/// intentionally, so callers can construct adversarial integration fixtures.
pub fn encode_records(records: &[Record], proofs: &[CommitSignature]) -> Vec<u8> {
    batch_bytes(records, proofs).expect("fixture batch serialization failed")
}

/// Encode a loose record without checking its payload commitment or signature.
pub fn encode_loose(record: &Record, signature: Signature) -> Vec<u8> {
    loose_bytes(record, signature).expect("fixture loose serialization failed")
}

impl SyncCodec<Write> for BatchCodec {
    type Error = io::Error;

    fn encode_commit(&mut self, commit: SignedCommitRef<'_, Write>) -> io::Result<Vec<u8>> {
        self.loose_encodes += 1;
        loose_bytes(
            &Record {
                meta: commit.commit.meta.clone(),
                payload: commit.commit.payload.clone(),
            },
            *commit.signature,
        )
    }

    fn encode_fragment(&mut self, fragment: FragmentRef<'_, Write>) -> io::Result<Vec<u8>> {
        self.fragment_encodes += 1;
        let records: Vec<_> = fragment
            .commits()
            .map(|commit| Record {
                meta: commit.meta.clone(),
                payload: commit.payload.clone(),
            })
            .collect();
        let proofs: Vec<_> = fragment
            .signatures()
            .map(|proof| CommitSignature {
                author: proof.author,
                commit: proof.commit,
                signature: proof.signature,
            })
            .collect();
        // One joint encoding, never a concatenation of loose encodings.
        batch_bytes(&records, &proofs)
    }

    fn decode_commit(&mut self, bytes: &[u8]) -> io::Result<DecodedLooseCommit<Write>> {
        let wire: LooseWire = bcs::from_bytes(bytes).map_err(invalid_data)?;
        Ok(DecodedLooseCommit {
            commit: DecodedCommit {
                contents: CommitContents {
                    author: Author::try_from(wire.author.as_slice()).map_err(invalid_data)?,
                    parents: wire.parents.into_iter().map(Into::into).collect(),
                    payload: BlobHash::from(wire.payload_hash),
                },
                payload: wire.payload,
            },
            signature: signature(wire.signature)?,
        })
    }

    fn decode_fragment(&mut self, bytes: &[u8]) -> io::Result<DecodedFragment<Write>> {
        let wire: BatchWire = bcs::from_bytes(bytes).map_err(invalid_data)?;
        let count = wire.commit_authors.len();
        if [
            wire.parents.len(),
            wire.payload_hashes.len(),
            wire.keys.len(),
            wire.values.len(),
            wire.nonces.len(),
        ]
        .into_iter()
        .any(|len| len != count)
        {
            return Err(invalid_data("batch columns have different lengths"));
        }
        let mut commits = Vec::with_capacity(count);
        for index in 0..count {
            let author = table_entry(&wire.authors, wire.commit_authors[index])?;
            commits.push(DecodedCommit {
                contents: CommitContents {
                    author: Author::try_from(author.as_slice()).map_err(invalid_data)?,
                    parents: wire.parents[index]
                        .iter()
                        .copied()
                        .map(Into::into)
                        .collect(),
                    payload: BlobHash::from(wire.payload_hashes[index]),
                },
                payload: Write {
                    key: table_entry(&wire.strings, wire.keys[index])?.clone(),
                    value: table_entry(&wire.strings, wire.values[index])?.clone(),
                    nonce: wire.nonces[index],
                },
            });
        }
        let mut signatures = Vec::with_capacity(wire.proofs.len());
        for proof in wire.proofs {
            let author = table_entry(&wire.authors, proof.author)?;
            signatures.push(CommitSignature {
                author: Author::try_from(author.as_slice()).map_err(invalid_data)?,
                commit: proof.commit.into(),
                signature: signature(proof.signature)?,
            });
        }
        Ok(DecodedFragment {
            commits,
            signatures,
        })
    }

    fn canonical_payload(&mut self, payload: &Write) -> io::Result<Vec<u8>> {
        bcs::to_bytes(payload).map_err(invalid_data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use stockfoil::{CommitHash, CommitRef};

    fn record(key: &SigningKey, payload: Write, parents: Vec<CommitHash>) -> Record {
        Record {
            meta: CommitMeta::new(CommitContents {
                author: Author::try_from(key.verifying_key().as_bytes().as_slice()).unwrap(),
                parents,
                payload: BlobHash::of_contents(&bcs::to_bytes(&payload).unwrap()),
            }),
            payload,
        }
    }

    fn proof(key: &SigningKey, record: &Record) -> CommitSignature {
        CommitSignature {
            author: *record.meta.author(),
            commit: *record.meta.hash(),
            signature: Signature::from(key.sign(record.meta.hash().as_bytes()).to_bytes()),
        }
    }

    #[test]
    fn joint_columns_preserve_hashes_payloads_parent_order_and_proofs() {
        let alice = SigningKey::from_bytes(&[1; 32]);
        let bob = SigningKey::from_bytes(&[2; 32]);
        let payload = Write {
            key: "shared".into(),
            value: "shared".into(),
            nonce: 0,
        };
        let first = record(&alice, payload.clone(), vec![]);
        let second = record(
            &bob,
            Write {
                nonce: u64::MAX,
                ..payload.clone()
            },
            vec![],
        );
        let third = record(
            &alice,
            Write {
                value: "\0Unicode: λ".into(),
                nonce: 7,
                ..payload
            },
            vec![*second.meta.hash(), *first.meta.hash(), *second.meta.hash()],
        );
        let proofs = vec![
            proof(&alice, &third),
            proof(&bob, &second),
            proof(&alice, &first),
        ];
        let records = vec![first, second, third];
        let bytes = encode_records(&records, &proofs);
        let wire: BatchWire = bcs::from_bytes(&bytes).unwrap();
        assert_eq!(wire.authors.len(), 2);
        assert_eq!(wire.strings.len(), 2);
        let mut codec = BatchCodec::default();
        let decoded = codec.decode_fragment(&bytes).unwrap();
        assert_eq!(decoded.commits.len(), records.len());
        for (decoded, original) in decoded.commits.into_iter().zip(&records) {
            assert_eq!(decoded.payload, original.payload);
            assert_eq!(decoded.contents.parents, original.meta.parents());
            let canonical = codec.canonical_payload(&decoded.payload).unwrap();
            assert_eq!(canonical, bcs::to_bytes(&original.payload).unwrap());
            assert!(decoded.contents.payload == BlobHash::of_contents(&canonical));
            assert_eq!(
                CommitMeta::new(decoded.contents).hash(),
                original.meta.hash()
            );
        }
        assert_eq!(decoded.signatures.len(), proofs.len());
        for (decoded, original) in decoded.signatures.iter().zip(&proofs) {
            assert_eq!(decoded.author, original.author);
            assert_eq!(decoded.commit, original.commit);
            assert_eq!(decoded.signature, original.signature);
            let key = ed25519_dalek::VerifyingKey::from_bytes(decoded.author.as_array()).unwrap();
            key.verify_strict(
                decoded.commit.as_bytes(),
                &ed25519_dalek::Signature::from_bytes(&decoded.signature.into()),
            )
            .unwrap();
        }
        assert_eq!((codec.loose_encodes, codec.fragment_encodes), (0, 0));
    }

    #[test]
    fn loose_roundtrip_and_counter() {
        let key = SigningKey::from_bytes(&[3; 32]);
        let record = record(
            &key,
            Write {
                key: "".into(),
                value: "v".into(),
                nonce: 3,
            },
            vec![],
        );
        let signature = proof(&key, &record).signature;
        let mut codec = BatchCodec::default();
        let bytes = codec
            .encode_commit(SignedCommitRef {
                commit: CommitRef {
                    meta: &record.meta,
                    payload: &record.payload,
                },
                signature: &signature,
            })
            .unwrap();
        assert_eq!(bytes, encode_loose(&record, signature));
        let decoded = codec.decode_commit(&bytes).unwrap();
        assert_eq!(decoded.commit.payload, record.payload);
        assert_eq!(
            CommitMeta::new(decoded.commit.contents).hash(),
            record.meta.hash()
        );
        assert_eq!(decoded.signature, signature);
        assert_eq!((codec.loose_encodes, codec.fragment_encodes), (1, 0));
    }

    #[test]
    fn empty_and_unauthenticated_batches_are_representable() {
        let mut codec = BatchCodec::default();
        let empty = codec.decode_fragment(&encode_records(&[], &[])).unwrap();
        assert!(empty.commits.is_empty());
        assert!(empty.signatures.is_empty());
        let key = SigningKey::from_bytes(&[4; 32]);
        let mut record = record(
            &key,
            Write {
                key: "k".into(),
                value: "v".into(),
                nonce: 4,
            },
            vec![],
        );
        let proof = proof(&key, &record);
        record.payload.value = "tampered".into();
        let decoded = codec
            .decode_fragment(&encode_records(&[record], &[proof]))
            .unwrap();
        let canonical = codec
            .canonical_payload(&decoded.commits[0].payload)
            .unwrap();
        assert!(decoded.commits[0].contents.payload != BlobHash::of_contents(&canonical));
        let proof_only = CommitSignature {
            author: Author::try_from(key.verifying_key().as_bytes().as_slice()).unwrap(),
            commit: CommitHash::from([99; 32]),
            signature: Signature::from(key.sign(&[99; 32]).to_bytes()),
        };
        let decoded = codec
            .decode_fragment(&encode_records(&[], &[proof_only]))
            .unwrap();
        assert!(decoded.commits.is_empty());
        assert_eq!(decoded.signatures.len(), 1);
    }

    #[test]
    fn malformed_wire_data_is_invalid_data() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let record = record(
            &key,
            Write {
                key: "k".into(),
                value: "v".into(),
                nonce: 5,
            },
            vec![],
        );
        let signature = proof(&key, &record).signature;
        let valid = encode_records(std::slice::from_ref(&record), &[proof(&key, &record)]);
        let mut codec = BatchCodec::default();
        for case in 0..7 {
            let mut wire: BatchWire = bcs::from_bytes(&valid).unwrap();
            match case {
                0 => {
                    wire.nonces.pop();
                }
                1 => wire.commit_authors[0] = u32::MAX,
                2 => wire.keys[0] = u32::MAX,
                3 => wire.values[0] = u32::MAX,
                4 => wire.proofs[0].author = u32::MAX,
                5 => {
                    wire.proofs[0].signature.pop();
                }
                _ => wire.proofs[0].signature.push(0),
            }
            let bytes = bcs::to_bytes(&wire).unwrap();
            assert_eq!(
                codec.decode_fragment(&bytes).err().unwrap().kind(),
                io::ErrorKind::InvalidData
            );
        }
        for length in [0, 63, 65] {
            let mut wire: LooseWire = bcs::from_bytes(&encode_loose(&record, signature)).unwrap();
            wire.signature = vec![0; length];
            assert_eq!(
                codec
                    .decode_commit(&bcs::to_bytes(&wire).unwrap())
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::InvalidData
            );
        }
        let mut trailing = valid.clone();
        trailing.push(0);
        for bytes in [&[][..], &valid[..valid.len() - 1], trailing.as_slice()] {
            assert_eq!(
                codec.decode_fragment(bytes).err().unwrap().kind(),
                io::ErrorKind::InvalidData
            );
        }
    }
}
