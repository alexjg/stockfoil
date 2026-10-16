//! Signature evidence: Ed25519 checks over commit hashes, and the rule that every
//! member of a bundle is covered by a signature of its own author.
use crate::{Author, BundleMeta, CommitHash, Error, ParentRef};
use std::collections::{BTreeMap, BTreeSet};

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
/// each, skipping members already marked in `visited`. Returns how many were
/// visited.
pub(crate) fn reach(
    meta: &BundleMeta,
    visited: &mut [bool],
    starts: impl IntoIterator<Item = usize>,
    mut visit: impl FnMut(usize),
) -> usize {
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
/// author signed. Returns the evidence rows that cover the bundle, in order:
/// for each member author, its signatures that no later signature of theirs
/// already reaches. Rows that cover nothing (duplicates, signatures below
/// another by the same signer, signers who author no member) are left out.
/// Fails with the first uncovered `(author, member)`.
pub(crate) fn coverage(meta: &BundleMeta) -> Result<Vec<usize>, (Author, usize)> {
    let authors: BTreeSet<Author> = meta.shape.members.iter().map(|m| m.author).collect();
    let mut rows: BTreeMap<Author, Vec<usize>> = BTreeMap::new();
    for (row, e) in meta.evidence.iter().enumerate() {
        if authors.contains(&e.author) {
            rows.entry(e.author).or_default().push(row);
        }
    }
    let mut covered = vec![false; meta.len()];
    let mut needed = Vec::new();
    for (author, mut rows) in rows {
        // Parents precede children, so no target is an ancestor of a later one.
        rows.sort_by_key(|&row| std::cmp::Reverse(meta.evidence[row].member));
        let mut visited = vec![false; meta.len()];
        for row in rows {
            let target = meta.evidence[row].member;
            if visited[target] {
                continue;
            }
            needed.push(row);
            reach(meta, &mut visited, [target], |i| {
                if meta.shape.members[i].author == author {
                    covered[i] = true;
                }
            });
        }
    }
    match covered.iter().position(|c| !c) {
        Some(i) => Err((meta.shape.members[i].author, i)),
        None => {
            needed.sort_unstable();
            Ok(needed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Evidence, FragmentShape, ItemId, MemberShape, Signature};

    #[test]
    fn coverage_keeps_only_rows_that_cover_something() {
        let [alice, bob, carol] = [1, 2, 3].map(|i| Author::try_from(&[i; 32][..]).unwrap());
        let member = |author, parents: &[usize]| MemberShape {
            author,
            parents: parents.iter().map(|&j| ParentRef::Member(j)).collect(),
        };
        // 0 alice <- 1 bob <- {2 alice, 3 alice} <- 4 bob
        let members = vec![
            member(alice, &[]),
            member(bob, &[0]),
            member(alice, &[1]),
            member(alice, &[1]),
            member(bob, &[2, 3]),
        ];
        let rows = |rows: &[(Author, usize)]| BundleMeta {
            id: ItemId::Commit(CommitHash::from([0; 32])),
            shape: FragmentShape {
                members: members.clone(),
            },
            evidence: rows
                .iter()
                .map(|&(author, member)| Evidence {
                    author,
                    member,
                    signature: Signature::from([0; 64]),
                })
                .collect(),
        };
        let meta = rows(&[
            (alice, 2),
            (alice, 0), // below alice's signature on 2
            (alice, 2), // a duplicate
            (alice, 3), // a concurrent tip, still needed
            (carol, 4), // carol authors no member
            (bob, 4),
            (bob, 1), // below bob's signature on 4
        ]);
        assert_eq!(coverage(&meta), Ok(vec![0, 3, 5]));
        let meta = rows(&[(alice, 2), (bob, 4), (carol, 3)]);
        assert_eq!(coverage(&meta), Err((alice, 3)));
    }
}
