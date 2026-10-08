//! Signature evidence: Ed25519 checks over commit hashes, and the rule that every
//! member of a bundle is covered by a signature of its own author.
use crate::{Author, BundleMeta, CommitHash, Error, ParentRef};
use std::collections::BTreeMap;

pub(crate) fn verify_signature(
    author: &Author,
    hash: &CommitHash,
    signature: &crate::Signature,
) -> Result<(), Error> {
    let key = ed25519_dalek::VerifyingKey::from_bytes(author.as_array())
        .map_err(|_| Error::InvalidPublicKey(*author))?;
    key.verify_strict(
        hash.as_bytes(),
        &ed25519_dalek::Signature::from(<[u8; 64]>::from(*signature)),
    )
    .map_err(|_| Error::InvalidSignature {
        author: *author,
        member: 0,
    })
}

/// Visit members reachable from `starts` through internal parent edges, once
/// each. Returns how many were visited.
pub(crate) fn reach(
    meta: &BundleMeta,
    starts: impl IntoIterator<Item = usize>,
    mut visit: impl FnMut(usize),
) -> usize {
    let mut visited = vec![false; meta.len()];
    let mut stack: Vec<usize> = starts.into_iter().collect();
    let mut count = 0;
    while let Some(i) = stack.pop() {
        if std::mem::replace(&mut visited[i], true) {
            continue;
        }
        count += 1;
        visit(i);
        stack.extend(
            meta.shape.members[i]
                .parents
                .iter()
                .filter_map(|p| match p {
                    ParentRef::Member(j) => Some(*j),
                    ParentRef::External(_) => None,
                }),
        );
    }
    count
}

/// Every member must be an ancestor-or-self (within the bundle) of a member its
/// author signed. Returns the first uncovered `(author, member)`.
pub(crate) fn coverage(meta: &BundleMeta) -> Result<(), (Author, usize)> {
    let mut seeds: BTreeMap<Author, Vec<usize>> = BTreeMap::new();
    for e in &meta.evidence {
        seeds.entry(e.author).or_default().push(e.member);
    }
    let mut covered = vec![false; meta.len()];
    for (author, targets) in seeds {
        reach(meta, targets, |i| {
            if meta.shape.members[i].author == author {
                covered[i] = true;
            }
        });
    }
    match covered.iter().position(|c| !c) {
        Some(i) => Err((meta.shape.members[i].author, i)),
        None => Ok(()),
    }
}
