//! Owned transport values. No caller-provided hash or certificate is trusted.
use std::collections::{BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use stockfoil::{
    Author, BlobHash, CommitContents, CommitHash, CommitMeta, FragmentRange, IngestReport, ItemId,
    SignatureEvidence, UnverifiedItem, VerifiedItem, ViewPoint, WritePermission,
};

use crate::error::{self, Error};

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 15) as usize] as char);
    }
    out
}

fn unhex(value: &str) -> Result<[u8; 32], Error> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::input(
            "expected exactly 64 lowercase hexadecimal characters",
        ));
    }
    fn digit(b: u8) -> u8 {
        if b <= b'9' { b - b'0' } else { b - b'a' + 10 }
    }
    let mut out = [0; 32];
    for (i, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        out[i] = digit(pair[0]) * 16 + digit(pair[1]);
    }
    Ok(out)
}

pub(crate) fn hash(value: &str) -> Result<CommitHash, Error> {
    Ok(unhex(value)?.into())
}

pub(crate) fn author(value: &str) -> Result<Author, Error> {
    Author::try_from(unhex(value)?.as_slice()).map_err(Error::input)
}

pub(crate) fn hashes(values: Vec<String>) -> Result<Vec<CommitHash>, Error> {
    // Do not sort or deduplicate: parent order AND multiplicity are authenticated.
    values.iter().map(|value| hash(value)).collect()
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Commit {
    pub hash: String,
    pub author: String,
    pub parents: Vec<String>,
    pub payload_hash: String,
    #[serde(with = "serde_bytes")]
    pub payload: Vec<u8>,
}

impl Commit {
    pub fn from_meta(meta: &CommitMeta, payload: &[u8]) -> Self {
        Self {
            hash: hex(meta.hash().as_slice()),
            author: hex(meta.author().as_slice()),
            parents: meta.parents().iter().map(|h| hex(h.as_slice())).collect(),
            payload_hash: hex(meta.payload_hash().as_slice()),
            payload: payload.to_vec(),
        }
    }

    fn validate(self) -> Result<(CommitMeta, Vec<u8>), Error> {
        let claimed_hash = hash(&self.hash)?;
        let claimed_payload = BlobHash::from(unhex(&self.payload_hash)?);
        let contents = CommitContents {
            author: author(&self.author)?,
            parents: hashes(self.parents)?,
            payload: BlobHash::of_contents(&self.payload),
        };
        if contents.payload != claimed_payload {
            return Err(Error::new(
                "PayloadHashMismatch",
                "payload bytes do not match payloadHash",
            ));
        }
        let meta = CommitMeta::new(contents);
        if *meta.hash() != claimed_hash {
            return Err(Error::new(
                "CommitHashMismatch",
                "commit metadata does not match hash",
            ));
        }
        Ok((meta, self.payload))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Evidence {
    pub author: String,
    pub commit: String,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
}

impl Evidence {
    pub fn into_core(self) -> Result<SignatureEvidence, Error> {
        let signature: [u8; 64] = self
            .signature
            .try_into()
            .map_err(|_| Error::input("signature must contain exactly 64 bytes"))?;
        Ok(SignatureEvidence {
            author: author(&self.author)?,
            commit: hash(&self.commit)?,
            signature: signature.into(),
        })
    }
}

impl From<SignatureEvidence> for Evidence {
    fn from(value: SignatureEvidence) -> Self {
        Self {
            author: hex(value.author.as_slice()),
            commit: hex(value.commit.as_slice()),
            signature: value.signature.as_slice().to_vec(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum TransportItem {
    Commit {
        commit: Commit,
        signatures: Vec<Evidence>,
    },
    Fragment {
        head: String,
        boundary: Vec<String>,
        commits: Vec<Commit>,
        signatures: Vec<Evidence>,
        /// Tips of history authenticated by other items. Usually empty.
        #[serde(default)]
        borrowed: Vec<String>,
    },
}

pub(crate) struct PreparedItem {
    pub verified: VerifiedItem,
    pub payloads: HashMap<CommitHash, Vec<u8>>,
}

impl TransportItem {
    pub fn verify(self) -> Result<PreparedItem, Error> {
        let mut payloads = HashMap::new();
        let (item, fragment) = match self {
            Self::Commit { commit, signatures } => {
                let (meta, payload) = commit.validate()?;
                payloads.insert(*meta.hash(), payload);
                (
                    UnverifiedItem::Commit {
                        meta,
                        signatures: signatures
                            .into_iter()
                            .map(Evidence::into_core)
                            .collect::<Result<_, _>>()?,
                    },
                    false,
                )
            }
            Self::Fragment {
                head,
                boundary,
                commits,
                signatures,
                borrowed,
            } => {
                let head = hash(&head)?;
                let mut bounds = BTreeSet::new();
                for bound in hashes(boundary)? {
                    if !bounds.insert(bound) {
                        return Err(Error::new("InvalidFragment", "duplicate fragment boundary"));
                    }
                }
                let mut tips = BTreeSet::new();
                for tip in hashes(borrowed)? {
                    if !tips.insert(tip) {
                        return Err(Error::new("InvalidFragment", "duplicate borrowed tip"));
                    }
                }
                let mut members = Vec::with_capacity(commits.len());
                for commit in commits {
                    let (meta, payload) = commit.validate()?;
                    if payloads.insert(*meta.hash(), payload).is_some() {
                        return Err(Error::new("InvalidFragment", "duplicate fragment member"));
                    }
                    members.push(meta);
                }
                (
                    UnverifiedItem::Fragment {
                        range: FragmentRange {
                            head,
                            boundary: bounds,
                        },
                        commits: members,
                        signatures: signatures
                            .into_iter()
                            .map(Evidence::into_core)
                            .collect::<Result<_, _>>()?,
                        borrowed: tips,
                    },
                    true,
                )
            }
        };
        // Validate every byte and signature before calling graph.ingest, including
        // duplicates and items destined for the pending dependency queue.
        let verified = item
            .verify()
            .map_err(|e| error::verification(e, fragment))?;
        Ok(PreparedItem { verified, payloads })
    }

    pub fn from_core(
        item: UnverifiedItem,
        graph: &stockfoil::CommitGraph<Vec<u8>>,
    ) -> Result<Self, Error> {
        let commit = |meta: CommitMeta| {
            let (_, payload) = graph
                .publishable_commit(meta.hash())
                .ok_or_else(|| Error::new("InternalError", "exported member is not publishable"))?;
            Ok(Commit::from_meta(&meta, payload))
        };
        Ok(match item {
            UnverifiedItem::Commit { meta, signatures } => Self::Commit {
                commit: commit(meta)?,
                signatures: signatures.into_iter().map(Into::into).collect(),
            },
            UnverifiedItem::Fragment {
                range,
                commits,
                signatures,
                borrowed,
            } => Self::Fragment {
                head: hex(range.head.as_slice()),
                boundary: range.boundary.iter().map(|h| hex(h.as_slice())).collect(),
                commits: commits
                    .into_iter()
                    .map(commit)
                    .collect::<Result<_, Error>>()?,
                signatures: signatures.into_iter().map(Into::into).collect(),
                borrowed: borrowed.iter().map(|h| hex(h.as_slice())).collect(),
            },
        })
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum Id {
    Commit { hash: String },
    Fragment { head: String, boundary: Vec<String> },
}

impl From<ItemId> for Id {
    fn from(value: ItemId) -> Self {
        match value {
            ItemId::Commit(hash) => Self::Commit {
                hash: hex(hash.as_slice()),
            },
            ItemId::Fragment(range) => Self::Fragment {
                head: hex(range.head.as_slice()),
                boundary: range.boundary.iter().map(|h| hex(h.as_slice())).collect(),
            },
        }
    }
}

#[derive(Serialize)]
pub(crate) struct Rejection {
    item: Id,
    error: Error,
}

#[derive(Serialize)]
pub(crate) struct Report {
    admitted: Vec<Id>,
    rejected: Vec<Rejection>,
    pending: usize,
}

impl From<IngestReport> for Report {
    fn from(value: IngestReport) -> Self {
        Self {
            admitted: value.admitted.into_iter().map(Into::into).collect(),
            rejected: value
                .rejected
                .into_iter()
                .map(|r| Rejection {
                    item: r.item.into(),
                    error: r.error.into(),
                })
                .collect(),
            pending: value.pending,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum SimplePermission {
    Unbounded,
    Denied,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Permission {
    Simple(SimplePermission),
    Bounded { through: Vec<String> },
}

pub(crate) type Permissions = HashMap<String, Permission>;

pub(crate) fn permissions(values: Permissions) -> Result<HashMap<Author, WritePermission>, Error> {
    values
        .into_iter()
        .map(|(key, value)| {
            Ok((
                author(&key)?,
                match value {
                    Permission::Simple(SimplePermission::Unbounded) => WritePermission::Unbounded,
                    Permission::Simple(SimplePermission::Denied) => WritePermission::Denied,
                    Permission::Bounded { through } => WritePermission::Bounded(hashes(through)?),
                },
            ))
        })
        .collect()
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct View {
    heads: Vec<String>,
    write_permissions: Permissions,
}

impl View {
    pub fn into_core(self) -> Result<ViewPoint, Error> {
        Ok(ViewPoint::from_parts(
            hashes(self.heads)?,
            permissions(self.write_permissions)?,
        ))
    }
}

impl From<ViewPoint> for View {
    fn from(value: ViewPoint) -> Self {
        Self {
            heads: value.heads().iter().map(|h| hex(h.as_slice())).collect(),
            write_permissions: value
                .write_permissions()
                .iter()
                .map(|(key, value)| {
                    (
                        hex(key.as_slice()),
                        match value {
                            WritePermission::Unbounded => {
                                Permission::Simple(SimplePermission::Unbounded)
                            }
                            WritePermission::Denied => Permission::Simple(SimplePermission::Denied),
                            WritePermission::Bounded(hashes) => Permission::Bounded {
                                through: hashes.iter().map(|h| hex(h.as_slice())).collect(),
                            },
                        },
                    )
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Change {
    pub commit: Commit,
    pub newly_delivered: bool,
    pub visibility: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(parents: Vec<CommitHash>) -> CommitMeta {
        CommitMeta::new(CommitContents {
            parents,
            author: Author::try_from([1; 32].as_slice()).unwrap(),
            payload: BlobHash::of_contents(b"payload"),
        })
    }

    #[test]
    fn canonical_identifiers_only() {
        assert_eq!(hex(&unhex(&"ab".repeat(32)).unwrap()), "ab".repeat(32));
        for invalid in [
            "AB".repeat(32),
            "0".repeat(63),
            "0".repeat(65),
            "gg".repeat(32),
            "é".repeat(32),
        ] {
            assert_eq!(hash(&invalid).unwrap_err().code, "InvalidInput");
        }
    }

    #[test]
    fn validates_bytes_and_exact_parent_sequence() {
        let parents = vec![[2; 32].into(), [1; 32].into(), [2; 32].into()];
        let original = meta(parents.clone());
        let (validated, _) = Commit::from_meta(&original, b"payload").validate().unwrap();
        assert_eq!(validated.parents(), parents);
        let mut commit = Commit::from_meta(&original, b"payload");
        commit.parents.swap(0, 1);
        assert_eq!(commit.validate().unwrap_err().code, "CommitHashMismatch");
        let commit = Commit::from_meta(&original, b"tampered");
        assert_eq!(commit.validate().unwrap_err().code, "PayloadHashMismatch");
    }

    #[test]
    fn rejects_duplicate_fragment_entries_before_verification() {
        let original = meta(Vec::new());
        let h = hex(original.hash().as_slice());
        let duplicate_members = TransportItem::Fragment {
            head: h.clone(),
            boundary: vec![],
            signatures: vec![],
            borrowed: vec![],
            commits: vec![
                Commit::from_meta(&original, b"payload"),
                Commit::from_meta(&original, b"payload"),
            ],
        };
        assert_eq!(
            duplicate_members.verify().err().unwrap().message,
            "duplicate fragment member"
        );
        let duplicate_boundaries = TransportItem::Fragment {
            head: h.clone(),
            boundary: vec![h.clone(), h],
            commits: vec![],
            signatures: vec![],
            borrowed: vec![],
        };
        assert_eq!(
            duplicate_boundaries.verify().err().unwrap().message,
            "duplicate fragment boundary"
        );
    }
}
