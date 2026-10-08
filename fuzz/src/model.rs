//! A naive whole-history reference model. Everything here is computed by
//! brute force from the public topology (`traverse`, `parents`, `lookup`) so it
//! shares no logic with stockfoil's clocks and chains.
use crate::codec::depth;
use std::collections::{BTreeSet, HashMap};
use stockfoil::*;

/// A fixed-size bitset over node positions.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Set(Vec<u64>);

impl Set {
    pub fn new(n: usize) -> Self {
        Set(vec![0; n.div_ceil(64)])
    }
    pub fn insert(&mut self, i: usize) {
        self.0[i / 64] |= 1 << (i % 64);
    }
    pub fn contains(&self, i: usize) -> bool {
        self.0[i / 64] & (1 << (i % 64)) != 0
    }
    pub fn union(&mut self, other: &Set) {
        for (a, b) in self.0.iter_mut().zip(&other.0) {
            *a |= b;
        }
    }
    pub fn minus(&mut self, other: &Set) {
        for (a, b) in self.0.iter_mut().zip(&other.0) {
            *a &= !b;
        }
    }
    pub fn iter(&self, n: usize) -> impl Iterator<Item = usize> + '_ {
        (0..n).filter(|&i| self.contains(i))
    }
}

pub struct Dag {
    pub hashes: Vec<CommitHash>,
    pub index: HashMap<CommitHash, usize>,
    pub parents: Vec<Vec<usize>>,
    pub authors: Vec<Author>,
    pub admitted: Vec<bool>,
    pub payload_ids: Vec<PayloadId>,
    /// Inclusive ancestry of each node.
    pub anc: Vec<Set>,
}

impl Dag {
    /// Snapshot a history. Panics if `traverse` is not parents-first or a
    /// parent is undelivered.
    pub fn of(h: &History) -> Dag {
        let hashes: Vec<CommitHash> = h.traverse().collect();
        let n = hashes.len();
        assert_eq!(n, h.len(), "traverse length differs from len()");
        let mut index = HashMap::new();
        let mut parents = Vec::with_capacity(n);
        let mut authors = Vec::with_capacity(n);
        let mut admitted = Vec::with_capacity(n);
        let mut payload_ids = Vec::with_capacity(n);
        let mut anc: Vec<Set> = Vec::with_capacity(n);
        for (i, hash) in hashes.iter().enumerate() {
            assert!(index.insert(*hash, i).is_none(), "traverse repeats a commit");
            let ps: Vec<usize> = h
                .parents(hash)
                .expect("traversed commit has no parents entry")
                .map(|p| {
                    *index
                        .get(&p)
                        .unwrap_or_else(|| panic!("traverse is not parents-first or parent undelivered"))
                })
                .collect();
            let info = h.lookup(hash).expect("traversed commit not found");
            let mut a = Set::new(n);
            a.insert(i);
            for &p in &ps {
                a.union(&anc[p]);
            }
            anc.push(a);
            parents.push(ps);
            authors.push(info.author);
            admitted.push(info.admitted);
            payload_ids.push(info.payload);
        }
        Dag { hashes, index, parents, authors, admitted, payload_ids, anc }
    }

    pub fn len(&self) -> usize {
        self.hashes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.hashes.is_empty()
    }

    /// Inclusive ancestry of a set of nodes.
    pub fn ancestry(&self, heads: impl IntoIterator<Item = usize>) -> Set {
        let mut s = Set::new(self.len());
        for h in heads {
            s.union(&self.anc[h]);
        }
        s
    }

    pub fn ancestry_of_hashes(&self, heads: &[CommitHash]) -> Set {
        self.ancestry(heads.iter().map(|h| self.index[h]))
    }

    /// Maximal elements of `set` (members with no child in `set`).
    pub fn maximal(&self, set: impl Fn(usize) -> bool) -> BTreeSet<CommitHash> {
        let mut has_child = vec![false; self.len()];
        for i in 0..self.len() {
            if set(i) {
                for &p in &self.parents[i] {
                    has_child[p] = true;
                }
            }
        }
        (0..self.len())
            .filter(|&i| set(i) && !has_child[i])
            .map(|i| self.hashes[i])
            .collect()
    }

    /// Participation of every node in a view, computed naively.
    pub fn participation(&self, view: &ViewPoint) -> Set {
        let history = self.ancestry_of_hashes(view.heads());
        let mut out = Set::new(self.len());
        for i in history.iter(self.len()) {
            let allowed = match view.write_permissions().get(&self.authors[i]) {
                Some(WritePermission::Unbounded) => true,
                Some(WritePermission::Bounded(frontier)) => frontier.iter().any(|f| {
                    self.index
                        .get(f)
                        .is_some_and(|&f| history.contains(f) && self.anc[f].contains(i))
                }),
                Some(WritePermission::Denied) | None => false,
            };
            if allowed {
                out.insert(i);
            }
        }
        out
    }

    /// Naive canonical fragment for `head`: boundary stops at depth >= level
    /// and the members between.
    pub fn canonical_fragment(&self, head: usize) -> (BTreeSet<CommitHash>, BTreeSet<CommitHash>) {
        let level = depth(&self.hashes[head]);
        let mut boundary = BTreeSet::new();
        let mut seen = vec![false; self.len()];
        let mut stack = vec![head];
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            if i != head && depth(&self.hashes[i]) >= level {
                boundary.insert(i);
            } else {
                stack.extend(&self.parents[i]);
            }
        }
        let mut members = self.anc[head].clone();
        members.minus(&self.ancestry(boundary.iter().copied()));
        (
            boundary.into_iter().map(|i| self.hashes[i]).collect(),
            members.iter(self.len()).map(|i| self.hashes[i]).collect(),
        )
    }

    /// Check every structural invariant of `h` against this snapshot.
    pub fn check(&self, h: &History) {
        let n = self.len();
        // Admission is parent-closed.
        for i in 0..n {
            if self.admitted[i] {
                for &p in &self.parents[i] {
                    assert!(self.admitted[p], "admitted commit has an unadmitted parent");
                }
            }
            assert_eq!(h.is_admitted(&self.hashes[i]), self.admitted[i]);
        }
        let delivered = self.maximal(|_| true);
        let heads: BTreeSet<_> = h.heads(Heads::Delivered).collect();
        assert_eq!(heads, delivered, "heads(Delivered) differ from maximal delivered commits");
        let admitted = self.maximal(|i| self.admitted[i]);
        let heads: BTreeSet<_> = h.heads(Heads::Admitted).collect();
        assert_eq!(heads, admitted, "heads(Admitted) differ from maximal admitted commits");
        let drafts: Vec<_> = h.drafts().collect();
        let expected: Vec<_> = (0..n).filter(|&i| !self.admitted[i]).map(|i| self.hashes[i]).collect();
        assert_eq!(drafts, expected, "drafts() differ from unadmitted commits in order");
        // Ancestry, exhaustively for small histories, else sampled.
        let step = if n <= 96 { 1 } else { n / 64 };
        for a in (0..n).step_by(step) {
            for d in 0..n {
                let expected = a != d && self.anc[d].contains(a);
                assert_eq!(
                    h.is_ancestor(&self.hashes[a], &self.hashes[d]),
                    expected,
                    "is_ancestor({a}, {d}) differs from naive DFS"
                );
            }
        }
    }

    /// Check one admitted fragment against the naive canonical placement.
    pub fn check_fragment(&self, range: &FragmentRange) {
        let head = self.index[&range.head];
        let (boundary, members) = self.canonical_fragment(head);
        assert_eq!(boundary, range.boundary, "admitted fragment boundary is not canonical");
        assert!(depth(&range.head) > 0, "admitted fragment head has depth 0");
        for m in &members {
            assert!(self.admitted[self.index[m]], "fragment member not admitted");
        }
    }

    /// Admitted commit hashes.
    pub fn admitted_set(&self) -> BTreeSet<CommitHash> {
        (0..self.len()).filter(|&i| self.admitted[i]).map(|i| self.hashes[i]).collect()
    }
}

/// Expected change records from `cursor` to `target`, naively.
pub fn expected_changes(
    dag: &Dag,
    cursor: &MaterializationCursor,
    target: &ViewPoint,
) -> Vec<(CommitHash, PayloadId, bool, Vis)> {
    let delivered_before = dag.ancestry_of_hashes(cursor.delivery_heads());
    let before = dag.participation(cursor.view_point());
    let after = dag.participation(target);
    (0..dag.len())
        .filter_map(|i| {
            let newly = !delivered_before.contains(i);
            let (b, a) = (before.contains(i), after.contains(i));
            (newly || b != a).then(|| {
                let vis = match (b, a) {
                    (true, true) => Vis::Both,
                    (true, false) => Vis::Before,
                    (false, true) => Vis::After,
                    (false, false) => Vis::Neither,
                };
                (dag.hashes[i], dag.payload_ids[i], newly, vis)
            })
        })
        .collect()
}

/// Run a full change traversal in batches of `batch`, check it against the
/// model, and return the completed cursor.
pub fn check_changes(
    h: &History,
    dag: &Dag,
    cursor: &MaterializationCursor,
    target: &ViewPoint,
    batch: usize,
) -> MaterializationCursor {
    let mut changes = h.changes(cursor, target).expect("changes with delivered heads failed");
    let total = changes.remaining();
    let mut out = Vec::new();
    let mut calls = 0;
    let next = loop {
        calls += 1;
        assert!(calls <= total + 2, "next_changes does not finish");
        let before = out.len();
        match h.next_changes(&mut changes, &mut out, batch).expect("next_changes failed") {
            Batch::More => assert!(out.len() > before, "non-final batch made no progress"),
            Batch::Complete(c) => {
                assert!(out.len() - before <= batch, "batch larger than max");
                break c;
            }
        }
    };
    assert_eq!(changes.remaining(), 0);
    let mut seen = std::collections::HashSet::new();
    let mut got: Vec<_> = out
        .iter()
        .map(|c| {
            assert!(seen.insert(c.hash), "duplicate change record");
            (c.hash, c.payload, c.newly_delivered, c.visibility)
        })
        .collect();
    // Parents first: no record may be an ancestor of an earlier record.
    for (i, a) in got.iter().enumerate() {
        for b in &got[..i] {
            let (ai, bi) = (dag.index[&a.0], dag.index[&b.0]);
            assert!(!dag.anc[bi].contains(ai), "change records are not parents first");
        }
    }
    let expected = expected_changes(dag, cursor, target);
    got.sort_by_key(|r| dag.index[&r.0]);
    assert_eq!(got, expected, "change records differ from the naive model");
    let heads: Vec<_> = h.heads(Heads::Delivered).collect();
    assert_eq!(next.delivery_heads(), heads.as_slice(), "completed cursor delivery heads");
    assert_eq!(next.view_point(), target, "completed cursor view point");
    next
}

/// Check a participation diff against the model.
pub fn check_diff(h: &History, dag: &Dag, before: &ViewPoint, after: &ViewPoint) {
    let diff = h.diff(before, after).expect("diff with delivered heads failed");
    let (b, a) = (dag.participation(before), dag.participation(after));
    let mut expected = Vec::new();
    for i in 0..dag.len() {
        let vis = match (b.contains(i), a.contains(i)) {
            (true, true) => Vis::Both,
            (true, false) => Vis::Before,
            (false, true) => Vis::After,
            (false, false) => Vis::Neither,
        };
        assert_eq!(diff.visibility(&dag.hashes[i]).unwrap(), vis, "ViewDiff::visibility differs");
        match vis {
            Vis::Before => expected.push(CommitDiff { commit: dag.hashes[i], change: VisDiff::Before }),
            Vis::After => expected.push(CommitDiff { commit: dag.hashes[i], change: VisDiff::After }),
            _ => {}
        }
    }
    assert_eq!(diff.changed_commits(), expected, "ViewDiff::changed_commits differs");
}
