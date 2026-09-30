//! Owned JavaScript query, signature, and report values.
use std::collections::HashMap;

use crate::error::Error;
use serde::{Deserialize, Serialize};
use stockfoil::{
    Author, CommitHash, CommitMeta, IngestReport, ItemId, Signature, ViewPoint,
    WritePermission,
};

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
    // Parent order AND multiplicity are authenticated.
    values.iter().map(|value| hash(value)).collect()
}

#[derive(Serialize)]
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
}

#[derive(Deserialize)]
pub(crate) struct Evidence {
    pub author: String,
    pub commit: String,
    #[serde(with = "serde_bytes")]
    pub signature: Vec<u8>,
}
impl Evidence {
    pub fn into_core(self) -> Result<(Author, CommitHash, Signature), Error> {
        let signature: [u8; 64] = self
            .signature
            .try_into()
            .map_err(|_| Error::input("signature must contain exactly 64 bytes"))?;
        Ok((author(&self.author)?, hash(&self.commit)?, signature.into()))
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
