//! Signature evidence: Ed25519 checks over commit hashes, and the rule that every
//! member of a bundle is covered by a signature of its own author.
use crate::{Author, BundleMeta, CommitHash, Error, ParentRef};
use std::collections::BTreeSet;

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
/// author signed. Returns the evidence rows that cover the bundle, in order:
/// for each member author, its signatures that no later signature of theirs
/// already reaches. Rows that cover nothing (duplicates, signatures below
/// another by the same signer, signers who author no member) are left out.
/// Fails with an uncovered `(author, member)`.
pub(crate) fn coverage(meta: &BundleMeta) -> Result<Vec<usize>, (Author, usize)> {
    let mut coverage = Coverage::new(meta);
    loop {
        coverage.step(meta, usize::MAX)?;
        if let Some(rows) = coverage.rows() {
            return Ok(rows);
        }
    }
}

/// [`coverage`] in steps of bounded work.
///
/// Members are ordered parents first, so one pass from the last member to the
/// first carries a bit per author from each signature down to its ancestors.
/// Each pass handles 64 member authors, so the work is about the bundle's
/// members and parent edges for every 64 of them, whatever the shape.
pub(crate) struct Coverage {
    /// Distinct member authors, and each member's index into them.
    authors: Vec<Author>,
    author_of: Vec<usize>,
    /// Rows by member authors as `(author, target, row)`, ordered by pass, then
    /// target from last to first.
    rows: Vec<(usize, usize, usize)>,
    next_row: usize,
    pass: usize,
    /// The current pass has `0..remaining` left to visit.
    remaining: usize,
    masks: Vec<u64>,
    needed: Vec<usize>,
}

impl Coverage {
    pub fn new(meta: &BundleMeta) -> Self {
        let authors: Vec<Author> = meta
            .shape
            .members
            .iter()
            .map(|m| m.author)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let index = |author: &Author| authors.binary_search(author).ok();
        let author_of = meta
            .shape
            .members
            .iter()
            .map(|m| index(&m.author).unwrap())
            .collect();
        let mut rows: Vec<_> = meta
            .evidence
            .iter()
            .enumerate()
            .filter_map(|(row, e)| Some((index(&e.author)?, e.member, row)))
            .collect();
        rows.sort_unstable_by_key(|&(author, target, row)| {
            (author / 64, std::cmp::Reverse(target), row)
        });
        Self {
            authors,
            author_of,
            rows,
            next_row: 0,
            pass: 0,
            remaining: meta.len(),
            masks: vec![0; meta.len()],
            needed: Vec::new(),
        }
    }

    /// Do about `budget` units of work, one per member and parent edge
    /// visited, and return the work done.
    pub fn step(&mut self, meta: &BundleMeta, budget: usize) -> Result<usize, (Author, usize)> {
        let mut work = 0;
        while self.pass < self.passes() {
            while self.remaining > 0 {
                if work > 0 && work >= budget {
                    return Ok(work);
                }
                self.remaining -= 1;
                let member = self.remaining;
                // Read once per pass, so clearing it leaves the masks zeroed
                // for the next.
                let mut mask = std::mem::take(&mut self.masks[member]);
                while let Some(&(author, target, row)) = self.rows.get(self.next_row)
                    && author / 64 == self.pass
                    && target == member
                {
                    let bit = 1 << (author % 64);
                    if mask & bit == 0 {
                        mask |= bit;
                        self.needed.push(row);
                    }
                    self.next_row += 1;
                }
                let author = self.author_of[member];
                if author / 64 == self.pass && mask & (1 << (author % 64)) == 0 {
                    return Err((self.authors[author], member));
                }
                let parents = &meta.shape.members[member].parents;
                for parent in parents {
                    if let ParentRef::Member(j) = parent {
                        self.masks[*j] |= mask;
                    }
                }
                work += 1 + parents.len();
            }
            self.pass += 1;
            self.remaining = meta.len();
        }
        Ok(work)
    }

    /// After the last pass, the rows that cover the bundle (taken once).
    pub fn rows(&mut self) -> Option<Vec<usize>> {
        (self.pass == self.passes()).then(|| {
            self.needed.sort_unstable();
            std::mem::take(&mut self.needed)
        })
    }

    fn passes(&self) -> usize {
        self.authors.len().div_ceil(64)
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

    /// A chain over more authors than one pass handles, every member signed by
    /// its author, checked whole and in small steps.
    #[test]
    fn coverage_steps_through_many_authors() {
        let authors: Vec<_> = (0..150u8)
            .map(|i| Author::try_from(&[i; 32][..]).unwrap())
            .collect();
        let n = 600;
        let members = (0..n)
            .map(|i| MemberShape {
                author: authors[i * 7 % authors.len()],
                parents: (i > 0)
                    .then(|| ParentRef::Member(i - 1))
                    .into_iter()
                    .collect(),
            })
            .collect::<Vec<_>>();
        let mut last = std::collections::BTreeMap::new();
        for (i, m) in members.iter().enumerate() {
            last.insert(m.author, i);
        }
        // Every member is signed by its author; only the last ones count.
        let evidence = members
            .iter()
            .enumerate()
            .map(|(member, m)| Evidence {
                author: m.author,
                member,
                signature: Signature::from([0; 64]),
            })
            .collect();
        let meta = BundleMeta {
            id: ItemId::Commit(CommitHash::from([0; 32])),
            shape: FragmentShape { members },
            evidence,
        };
        let expected: Vec<usize> = last
            .values()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        assert_eq!(coverage(&meta), Ok(expected.clone()));
        let mut stepped = Coverage::new(&meta);
        let mut steps = 0;
        let rows = loop {
            assert!(stepped.step(&meta, 10).unwrap() <= 11);
            steps += 1;
            if let Some(rows) = stepped.rows() {
                break rows;
            }
        };
        assert_eq!(rows, expected);
        assert!(steps > 100);

        // Without the last author's signatures, the last member is uncovered.
        let mut meta = meta;
        let author = meta.shape.members[n - 1].author;
        meta.evidence.retain(|e| e.author != author);
        assert_eq!(coverage(&meta), Err((author, n - 1)));
    }
}
