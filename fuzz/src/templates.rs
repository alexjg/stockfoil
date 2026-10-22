//! Valid items exported from a real graph, built once per process.
//!
//! The graph has three authors, a repeated parent, merges, two depth-1
//! fragments (the second with a non-empty boundary), standalone commits, and a
//! depth-2 fragment that subsumes everything before it. Items are snapshotted
//! after each stage, so both inner and outer fragments are available.
use crate::codec::*;
use crate::host::{DriveConfig, Host, receive_all};
use std::collections::{BTreeSet, HashMap};
use std::sync::OnceLock;
use stockfoil::*;

#[derive(Clone, Debug)]
pub struct Item {
    pub id: ItemId,
    pub bytes: Vec<u8>,
    /// Member hashes in item order.
    pub hashes: Vec<CommitHash>,
    /// Boundaries and external parents.
    pub deps: BTreeSet<CommitHash>,
}

#[derive(Clone, Debug)]
pub struct CommitData {
    pub author: Author,
    pub parents: Vec<CommitHash>,
    pub payload: Vec<u8>,
}

pub struct Templates {
    /// Every distinct item, in order of first export.
    pub items: Vec<Item>,
    /// The inventory exported after each stage (indices into `items`).
    pub stages: Vec<Vec<usize>>,
    /// Every commit, parents first.
    pub order: Vec<CommitHash>,
    pub commits: HashMap<CommitHash, CommitData>,
}

pub fn templates() -> &'static Templates {
    static T: OnceLock<Templates> = OnceLock::new();
    T.get_or_init(build)
}

fn build() -> Templates {
    let (alice, bob, carol) = (key(1), key(2), key(3));
    let mut g = graph();
    let mut t = Templates {
        items: vec![],
        stages: vec![],
        order: vec![],
        commits: HashMap::new(),
    };
    let c = |g: &mut TestGraph, t: &mut Templates, k: &ed25519_dalek::SigningKey, parents: Vec<CommitHash>, payload: Vec<u8>| {
        let h = signed(g, k, parents.clone(), &payload);
        t.order.push(h);
        t.commits.insert(h, CommitData { author: author(k), parents, payload });
        h
    };
    let snapshot = |g: &mut TestGraph, t: &mut Templates| {
        let plan = g.export(&ExportCursor::new());
        let mut stage = vec![];
        for i in 0..plan.len() {
            let mut bytes = vec![];
            g.write_item(&plan, i, &mut bytes).unwrap();
            let pos = match t.items.iter().position(|it| it.bytes == bytes) {
                Some(p) => p,
                None => {
                    let item = g.history().export_item(&plan, i).unwrap();
                    let hashes: Vec<_> = item.members.iter().map(|(h, _)| *h).collect();
                    let mut deps: BTreeSet<_> = item
                        .meta
                        .members
                        .iter()
                        .flat_map(|m| &m.parents)
                        .filter_map(|p| match p {
                            ParentRef::External(h) => Some(*h),
                            ParentRef::Member(_) => None,
                        })
                        .collect();
                    if let ItemId::Fragment(r) = &item.meta.id {
                        deps.extend(&r.boundary);
                    }
                    t.items.push(Item { id: item.meta.id.clone(), bytes, hashes, deps });
                    t.items.len() - 1
                }
            };
            stage.push(pos);
        }
        t.stages.push(stage);
    };
    let a0 = c(&mut g, &mut t, &alice, vec![], b"root".to_vec());
    let a1 = c(&mut g, &mut t, &alice, vec![a0], b"a1".to_vec());
    let a2 = c(&mut g, &mut t, &alice, vec![a1], b"a2".repeat(40));
    snapshot(&mut g, &mut t);
    let h1 = c(&mut g, &mut t, &alice, vec![a2], grind(&author(&alice), &[a2], 1, b"h1"));
    snapshot(&mut g, &mut t);
    let b1 = c(&mut g, &mut t, &bob, vec![h1], b"b1".to_vec());
    let c1 = c(&mut g, &mut t, &carol, vec![h1], vec![]);
    let m = c(&mut g, &mut t, &carol, vec![b1, c1], b"merge".to_vec());
    let dup = c(&mut g, &mut t, &bob, vec![m, m], b"dup".to_vec());
    snapshot(&mut g, &mut t);
    let h2 = c(&mut g, &mut t, &bob, vec![dup], grind(&author(&bob), &[dup], 1, b"h2"));
    snapshot(&mut g, &mut t);
    let tail = c(&mut g, &mut t, &alice, vec![h2], b"tail".to_vec());
    let x = c(&mut g, &mut t, &carol, vec![tail, a1], b"x".to_vec());
    snapshot(&mut g, &mut t);
    let h3 = c(&mut g, &mut t, &alice, vec![x], grind(&author(&alice), &[x], 2, b"h3"));
    let _after = c(&mut g, &mut t, &bob, vec![h3], b"after".to_vec());
    snapshot(&mut g, &mut t);
    t
}

impl Templates {
    /// A history (plus host payload table) holding every item of `stage`.
    pub fn receiver(&self, stage: usize, host: &mut Host) -> History {
        let mut history = History::new();
        if stage > 0 {
            let items: Vec<_> = self.stages[(stage - 1).min(self.stages.len() - 1)]
                .iter()
                .map(|&i| self.items[i].bytes.clone())
                .collect();
            let waiting = receive_all(&mut history, host, &items, &DriveConfig::default());
            assert!(waiting.is_empty(), "template stage does not admit");
        }
        history
    }

    /// A history holding as many template items as possible without any of
    /// `exclude`'s members (or, if `only_item`, without just that item).
    pub fn receiver_without(&self, exclude: usize, only_item: bool, host: &mut Host) -> History {
        let mut history = History::new();
        let members: BTreeSet<_> = self.items[exclude].hashes.iter().collect();
        let items: Vec<_> = self
            .items
            .iter()
            .enumerate()
            .filter(|(i, it)| {
                *i != exclude && (only_item || !it.hashes.iter().any(|h| members.contains(h)))
            })
            .map(|(_, it)| it.bytes.clone())
            .collect();
        receive_all(&mut history, host, &items, &DriveConfig::default());
        history
    }
}
