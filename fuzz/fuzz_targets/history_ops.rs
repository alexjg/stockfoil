//! Random operation sequences on two `Graph<ChunkCodec, MemoryStorage>`
//! replicas: local commits by three keys on random parents (repeats allowed,
//! some ground to fragment depth), signing by the right or wrong key, partial
//! and reordered exports between replicas, change traversals and diffs with
//! random views and permissions, and full synchronization.
//!
//! Everything is checked against the naive model in `stockfoil_fuzz::model`:
//! parent-closed admission, heads, drafts, `is_ancestor`, change records,
//! diffs, `traverse_since`, payload bytes, and convergence after a full sync.
#![no_main]
use arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;
use std::collections::{HashMap, HashSet};
use stockfoil::*;
use stockfoil_fuzz::codec::*;
use stockfoil_fuzz::model::{Dag, check_changes, check_diff};

#[derive(Arbitrary, Debug)]
enum HeadSel {
    Delivered,
    Admitted,
    Some(Vec<u8>),
}

#[derive(Arbitrary, Debug)]
enum Perm {
    Missing,
    Denied,
    Unbounded,
    Bounded(Vec<u8>, bool),
}

#[derive(Arbitrary, Debug)]
enum Op {
    Commit { r: bool, author: u8, parents: Vec<u8>, payload: u8, deep: u8 },
    Sign { r: bool, target: u8, signer: u8, claimed: u8, corrupt: bool, any: bool },
    SignAll { r: bool },
    Send { from: bool, order: Vec<u8>, take: u8 },
    Retry { r: bool },
    Changes { r: bool, mat: bool, heads: HeadSel, perms: [Perm; 3], batch: u8 },
    Diff { r: bool, a: HeadSel, pa: [Perm; 3], b: HeadSel, pb: [Perm; 3] },
    SyncAll,
}

struct Replica {
    g: TestGraph,
    /// Stored items waiting for dependencies.
    pending: Vec<BlobHash>,
    mats: [MaterializationCursor; 2],
    /// What the other replica has acknowledged from this one.
    sent: ExportCursor,
    /// Drafts validly signed by their own author.
    self_signed: HashSet<CommitHash>,
}

impl Replica {
    fn new() -> Self {
        Replica {
            g: graph(),
            pending: vec![],
            mats: Default::default(),
            sent: ExportCursor::new(),
            self_signed: HashSet::new(),
        }
    }
    fn delivered(&self) -> Vec<CommitHash> {
        self.g.history().traverse().collect()
    }
}

struct World {
    reps: [Replica; 2],
    keys: [ed25519_dalek::SigningKey; 3],
    payloads: HashMap<CommitHash, Vec<u8>>,
    deep2: bool,
}

fn pick(list: &[CommitHash], picks: &[u8], max: usize) -> Vec<CommitHash> {
    if list.is_empty() {
        return vec![];
    }
    picks.iter().take(max).map(|p| list[*p as usize % list.len()]).collect()
}

impl World {
    fn view(&self, r: usize, heads: &HeadSel, perms: &[Perm; 3]) -> ViewPoint {
        let h = self.reps[r].g.history();
        let delivered = self.reps[r].delivered();
        let mut wp = WritePermissions::new();
        for (k, p) in self.keys.iter().zip(perms) {
            let p = match p {
                Perm::Missing => continue,
                Perm::Denied => WritePermission::Denied,
                Perm::Unbounded => WritePermission::Unbounded,
                Perm::Bounded(picks, unknown) => {
                    let mut f = pick(&delivered, picks, 3);
                    if *unknown {
                        f.push(CommitHash::from([0xab; 32]));
                    }
                    WritePermission::Bounded(f)
                }
            };
            wp.insert(author(k), p);
        }
        match heads {
            HeadSel::Delivered => {
                let v = h.view_point(&wp);
                assert_eq!(v.heads(), h.heads(Heads::Delivered).collect::<Vec<_>>().as_slice());
                v
            }
            HeadSel::Admitted => h.signed_view_point(&wp),
            HeadSel::Some(picks) => ViewPoint::from_parts(pick(&delivered, picks, 4), wp),
        }
    }

    fn apply(&mut self, op: &Op) {
        match op {
            Op::Commit { r, author: a, parents, payload, deep } => {
                let rep = &mut self.reps[*r as usize];
                let key = &self.keys[*a as usize % 3];
                let parents = pick(&rep.delivered(), parents, 4);
                let mut bytes = vec![*payload];
                if *deep % 8 == 1 {
                    bytes = grind(&author(key), &parents, 1, &bytes);
                } else if *deep == 255 && !self.deep2 {
                    self.deep2 = true;
                    bytes = grind(&author(key), &parents, 2, &bytes);
                }
                let epoch = rep.g.history().epoch();
                let existed = rep.g.history().lookup(&commit_hash(&parents, &author(key), &bytes));
                let p = rep.g.commit(author(key), parents.clone(), bytes.as_slice()).expect("local commit failed");
                assert_eq!(p.hash, commit_hash(&parents, &author(key), &bytes), "commit identity rule");
                assert_eq!(p.parents, parents);
                let info = rep.g.history().lookup(&p.hash).expect("commit not delivered");
                assert_eq!(info.author, author(key));
                match existed {
                    Some(prev) => {
                        assert_eq!(info, prev, "re-committing changed the commit");
                        assert_eq!(rep.g.history().epoch(), epoch, "duplicate commit changed the epoch");
                    }
                    None => assert!(!info.admitted && rep.g.history().epoch() > epoch),
                }
                self.payloads.insert(p.hash, bytes);
            }
            Op::Sign { r, target, signer, claimed, corrupt, any } => {
                let rep = &mut self.reps[*r as usize];
                let list: Vec<_> = if *any { rep.delivered() } else { rep.g.history().drafts().collect() };
                if list.is_empty() {
                    return;
                }
                let hash = list[*target as usize % list.len()];
                let signer = &self.keys[*signer as usize % 3];
                let claimed = author(&self.keys[*claimed as usize % 3]);
                let mut sig: [u8; 64] = sign(signer, &hash).into();
                if *corrupt {
                    sig[(*target as usize) % 64] ^= 1 << (*target % 8);
                }
                let admitted = rep.g.history().is_admitted(&hash);
                let epoch = rep.g.history().epoch();
                let result = rep.g.sign(claimed, hash, Signature::from(sig));
                if admitted {
                    assert_eq!(result, Err(Error::NotDraft(hash)));
                } else if *corrupt || claimed != author(signer) {
                    assert!(matches!(result, Err(Error::InvalidSignature { .. })), "{result:?}");
                    assert_eq!(rep.g.history().epoch(), epoch);
                } else {
                    let items = result.expect("valid signature rejected");
                    for id in &items {
                        assert!(rep.g.history().is_admitted(&id.head()), "reported admission not admitted");
                    }
                    if claimed == rep.g.history().lookup(&hash).unwrap().author {
                        rep.self_signed.insert(hash);
                    }
                }
            }
            Op::SignAll { r } => {
                let drafts: Vec<_> = self.reps[*r as usize].g.history().drafts().collect();
                for d in drafts {
                    let rep = &mut self.reps[*r as usize];
                    if rep.g.history().is_admitted(&d) {
                        continue;
                    }
                    let a = rep.g.history().lookup(&d).unwrap().author;
                    let key = self.keys.iter().find(|k| author(k) == a).unwrap();
                    rep.g.sign(a, d, sign(key, &d)).expect("own-author signature rejected");
                    rep.self_signed.insert(d);
                }
            }
            Op::Send { from, order, take } => self.send(*from as usize, order, *take),
            Op::Retry { r } => self.retry(*r as usize),
            Op::Changes { r, mat, heads, perms, batch } => {
                let target = self.view(*r as usize, heads, perms);
                let (rep, other) = (&self.reps[*r as usize], &self.reps[1 - *r as usize]);
                let h = rep.g.history();
                let cursor = rep.mats[*mat as usize].clone();
                let mut ch = h.changes(&cursor, &target).expect("changes failed");
                assert_eq!(
                    other.g.history().next_changes(&mut ch, &mut vec![], 1).unwrap_err(),
                    Error::WrongHistory
                );
                let dag = Dag::of(h);
                let next = check_changes(h, &dag, &cursor, &target, (*batch as usize % 9).max(1));
                let bytes = next.to_bytes();
                assert_eq!(MaterializationCursor::from_bytes(&bytes).as_ref(), Ok(&next));
                // traverse_since agrees with the model.
                let excluded = dag.ancestry_of_hashes(target.heads());
                let expected: Vec<_> = (0..dag.len()).filter(|&i| !excluded.contains(i)).map(|i| dag.hashes[i]).collect();
                let mut since = target.heads().to_vec();
                since.push(CommitHash::from([0xcd; 32]));
                assert_eq!(h.traverse_since(&since), expected, "traverse_since");
                self.reps[*r as usize].mats[*mat as usize] = next;
            }
            Op::Diff { r, a, pa, b, pb } => {
                let (va, vb) = (self.view(*r as usize, a, pa), self.view(*r as usize, b, pb));
                let h = self.reps[*r as usize].g.history();
                check_diff(h, &Dag::of(h), &va, &vb);
            }
            Op::SyncAll => self.sync_all(),
        }
    }

    fn send(&mut self, x: usize, order: &[u8], take: u8) {
        let y = 1 - x;
        let mut plan = self.reps[x].g.export(&self.reps[x].sent);
        let n = plan.len();
        let mut idx: Vec<usize> = (0..n).collect();
        for (i, o) in order.iter().enumerate().take(n) {
            idx.swap(i, i + *o as usize % (n - i));
        }
        let k = if take == 0 { n } else { take as usize % (n + 1) };
        for &i in &idx[..k] {
            let mut bytes = vec![];
            self.reps[x].g.write_item(&plan, i, &mut bytes).expect("planned item cannot be written");
            let hlen = wire::header_len(&bytes, &Limits::default()).unwrap().unwrap();
            let meta = wire::decode_header(&bytes[..hlen], &format(), &Limits::default()).unwrap();
            assert_eq!(Some(&meta.id), plan.id(i));
            match self.reps[y].g.receive(bytes.as_slice()) {
                Ok(receipt) => {
                    assert_eq!(Some(&receipt.id), plan.id(i));
                    for m in &receipt.members {
                        assert!(self.reps[y].g.history().is_admitted(&m.hash));
                    }
                    plan.acknowledge(i).expect("acknowledge a planned item");
                }
                Err(Error::MissingDependency(h)) => {
                    assert!(self.reps[x].g.history().is_admitted(&h), "waiting on something the sender lacks");
                    assert!(!self.reps[y].g.history().is_admitted(&h), "waiting on an admitted commit");
                    self.reps[y].pending.push(BlobHash::of_contents(&bytes));
                }
                Err(e) => panic!("honest item {:?} rejected: {e:?}", plan.id(i)),
            }
        }
        let cursor = plan.cursor();
        assert_eq!(ExportCursor::from_bytes(&cursor.to_bytes()).as_ref(), Ok(&cursor));
        self.reps[x].sent = cursor;
        self.retry(y);
    }

    fn retry(&mut self, r: usize) {
        loop {
            let pending = std::mem::take(&mut self.reps[r].pending);
            let before = pending.len();
            for blob in pending {
                match self.reps[r].g.admit(&blob) {
                    Ok(_) => {}
                    Err(Error::MissingDependency(_)) => self.reps[r].pending.push(blob),
                    Err(e) => panic!("stored honest item rejected on retry: {e:?}"),
                }
            }
            if self.reps[r].pending.len() == before {
                return;
            }
        }
    }

    fn sync_all(&mut self) {
        for _ in 0..16 {
            let before: Vec<_> = self.reps.iter().map(|r| r.g.history().epoch()).collect();
            self.send(0, &[], 0);
            self.send(1, &[], 0);
            let after: Vec<_> = self.reps.iter().map(|r| r.g.history().epoch()).collect();
            if before == after {
                break;
            }
        }
        for r in &self.reps {
            assert!(r.pending.is_empty(), "items still waiting after a full sync");
        }
        let (a, b) = (Dag::of(self.reps[0].g.history()), Dag::of(self.reps[1].g.history()));
        assert_eq!(a.admitted_set(), b.admitted_set(), "replicas did not converge");
    }

    fn check(&mut self) {
        for r in 0..2 {
            let h = self.reps[r].g.history();
            let dag = Dag::of(h);
            dag.check(h);
            for d in &self.reps[r].self_signed {
                let i = dag.index[d];
                if !dag.admitted[i] && dag.parents[i].iter().all(|&p| dag.admitted[p]) {
                    panic!("draft signed by its author with admitted parents is not admitted (depth {})", depth(d));
                }
            }
        }
    }

    fn check_payloads(&mut self) {
        for r in 0..2 {
            let hashes: Vec<_> = self.reps[r].delivered();
            for h in hashes {
                let bytes = self.reps[r].g.payload(&h).expect("payload unreadable");
                assert_eq!(&bytes, &self.payloads[&h], "payload bytes differ");
            }
        }
    }
}

/// Print every op and both histories (`STOCKFOIL_FUZZ_TRACE=1`).
fn trace() -> bool {
    static T: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *T.get_or_init(|| std::env::var_os("STOCKFOIL_FUZZ_TRACE").is_some())
}

impl World {
    fn dump(&self) {
        let name = |h: &CommitHash| format!("{:02x}{:02x}", h.as_bytes()[0], h.as_bytes()[1]);
        for (r, rep) in self.reps.iter().enumerate() {
            let h = rep.g.history();
            let mut line = format!("  replica {r}:");
            for c in h.traverse() {
                let info = h.lookup(&c).unwrap();
                let a = self.keys.iter().position(|k| author(k) == info.author).unwrap();
                let ps: Vec<_> = h.parents(&c).unwrap().map(|p| name(&p)).collect();
                line += &format!(
                    " {}[k{} d{}{} <-{}]",
                    name(&c),
                    a + 1,
                    depth(&c),
                    if info.admitted { "" } else { " draft" },
                    ps.join(",")
                );
            }
            eprintln!("{line}");
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let mut w = World {
        reps: [Replica::new(), Replica::new()],
        keys: [key(1), key(2), key(3)],
        payloads: HashMap::new(),
        deep2: false,
    };
    for _ in 0..64 {
        let Ok(op) = Op::arbitrary(&mut u) else { break };
        if trace() {
            eprintln!("op: {op:?}");
        }
        w.apply(&op);
        if trace() {
            w.dump();
        }
        w.check();
        if u.is_empty() {
            break;
        }
    }
    w.sync_all();
    w.check();
    w.check_payloads();
});
