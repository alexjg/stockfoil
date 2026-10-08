//! Stored item bytes received through a fake host with arbitrary read sizes,
//! decoder capacities, work quanta, and limits.
//!
//! Input: 9 config bytes, then either the raw item (mode % 4 != 3) or bytes
//! that drive a crafted, validly signed bundle (mode % 4 == 3).
//!
//! Invariants: no panic; admission terminates; a failed or waiting admission
//! leaves the history untouched; a successful one admits every member, keeps
//! the history invariants (checked against a naive model), places fragments
//! canonically, and is re-exportable into a fresh history. Crafted bundles are
//! additionally checked against an independent validity predicate in both
//! directions (invalid never admitted; valid and placeable always admitted).
#![no_main]
use arbitrary::{Result, Unstructured};
use libfuzzer_sys::fuzz_target;
use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use stockfoil::*;
use stockfoil_fuzz::codec::*;
use stockfoil_fuzz::host::*;
use stockfoil_fuzz::model::Dag;
use stockfoil_fuzz::templates::{Templates, templates};

const SIZES: &[usize] = &[1, 2, 3, 4, 5, 7, 8, 13, 16, 31, 64, 100, 255, 1000, 4096, 65536];

fn read_size(v: u8) -> usize {
    match v {
        // Regression: huge sizes once overflowed the header buffer bound.
        255 => usize::MAX - 8,
        254 => 0,
        _ => SIZES[v as usize % SIZES.len()],
    }
}

fn options(cfg: &[u8], crafted: bool) -> ReceiveOptions {
    let mut o = ReceiveOptions::new(format());
    o.read_size = read_size(cfg[1]);
    o.decode_capacity = if cfg[2] == 255 { 0 } else { SIZES[cfg[2] as usize % SIZES.len()] };
    o.quantum = [1, 2, 3, 16, 100, 1024, 16384, 65536, 256 * 1024, usize::MAX][cfg[3] as usize % 10];
    let l = cfg[6];
    if l != 0 && !crafted {
        o.limits = Limits {
            max_header_bytes: [32, 100, 300, 64 << 20][(l & 3) as usize],
            max_members: [1, 2, 4, 65536][(l >> 2 & 3) as usize],
            max_parents: [0, 1, 3, 1_000_000][(l >> 4 & 3) as usize],
            max_signatures: [0, 1, 2, 65536][(l >> 6) as usize],
            max_boundaries: [0, 1, 2, 65536][(l & 3) as usize],
            max_canonical_bytes: [0, 10, 100, u64::MAX][(l >> 2 & 3) as usize],
            max_item_bytes: [10, 200, 2000, u64::MAX][(l >> 4 & 3) as usize],
        };
    }
    if cfg[8] & 4 != 0 {
        o.format = FormatId::new("other").unwrap();
    }
    o
}

struct Receiver {
    history: History,
    host: Host,
}

thread_local! {
    /// Receivers whose history no admission has changed, by (stage, drafts).
    static CACHE: RefCell<HashMap<(usize, bool), Receiver>> = RefCell::new(HashMap::new());
}

fn receiver(t: &Templates, stage: usize, drafts: bool) -> Receiver {
    if let Some(r) = CACHE.with(|c| c.borrow_mut().remove(&(stage, drafts))) {
        return r;
    }
    let mut host = Host::default();
    let mut history = t.receiver(stage, &mut host);
    if drafts {
        // Unsigned local drafts identical to template commits the stage lacks.
        for h in &t.order {
            let c = &t.commits[h];
            if history.lookup(h).is_none() && c.parents.iter().all(|p| history.lookup(p).is_some()) {
                let mut b = history.begin_commit(c.author, c.parents.clone()).unwrap();
                b.update(&c.payload);
                host.payloads.push(c.payload.clone());
                history
                    .publish_commit(b.finish(), (host.payloads.len() - 1) as PayloadId)
                    .unwrap();
            }
        }
    }
    Receiver { history, host }
}

/// A bundle built from fuzz input, signed with real keys.
struct Crafted {
    bytes: Vec<u8>,
    meta: BundleMeta,
    hashes: Vec<CommitHash>,
    /// Every evidence signature is valid.
    signatures_valid: bool,
    /// Bytes are exactly the honest encoding of `meta` and the payloads.
    intact: bool,
}

fn craft(u: &mut Unstructured, t: &Templates) -> Result<Crafted> {
    let keys = [key(1), key(2), key(3)];
    let stranger = Author::try_from(&u.arbitrary::<[u8; 32]>()?[..]).unwrap();
    let mut pool: Vec<CommitHash> = t.order.clone();
    pool.push(CommitHash::from(u.arbitrary::<[u8; 32]>()?));
    let n = u.int_in_range(1..=6)?;
    let mut shape: Vec<MemberShape> = vec![];
    let mut hashes: Vec<CommitHash> = vec![];
    let mut payloads: Vec<Vec<u8>> = vec![];
    let mut key_of: Vec<Option<usize>> = vec![];
    for i in 0..n {
        let kind = u.int_in_range(0..=3)?;
        let (author, k, parents, payload) = if kind == 2 {
            let h = pool[u.choose_index(t.order.len())?];
            let c = &t.commits[&h];
            let parents = c
                .parents
                .iter()
                .map(|p| match hashes.iter().position(|m| m == p) {
                    Some(j) => ParentRef::Member(j),
                    None => ParentRef::External(*p),
                })
                .collect();
            let k = keys.iter().position(|k| author(k) == c.author);
            (c.author, k, parents, c.payload.clone())
        } else {
            let a = u.int_in_range(0..=3)?;
            let (author, k) = if a < 3 { (author(&keys[a]), Some(a)) } else { (stranger, None) };
            let mut parents = vec![];
            for _ in 0..u.int_in_range(0..=3)? {
                parents.push(match u.int_in_range(0..=2)? {
                    0 if i > 0 => ParentRef::Member(i - 1 - u.int_in_range(0..=i - 1)?),
                    2 => ParentRef::External(*pool.last().unwrap()),
                    _ => ParentRef::External(pool[u.choose_index(pool.len())?]),
                });
            }
            let len = u.int_in_range(0..=6)?;
            let mut payload = u.bytes(len)?.to_vec();
            if kind == 3 {
                let level = if u.ratio(1, 512)? { 2 } else { 1 };
                let resolved = resolve(&parents, &hashes);
                payload = grind(&author, &resolved, level, &payload);
            }
            (author, k, parents, payload)
        };
        let resolved = resolve(&parents, &hashes);
        hashes.push(commit_hash(&resolved, &author, &payload));
        shape.push(MemberShape { author, parents });
        payloads.push(payload);
        key_of.push(k);
    }
    let fragment = n > 1 || u.arbitrary::<bool>()?;
    let head = match u.int_in_range(0..=2)? {
        0 => {
            if fragment && depth(&hashes[n - 1]) == 0 {
                // Make the last member a plausible fragment head.
                let resolved = resolve(&shape[n - 1].parents, &hashes);
                payloads[n - 1] = grind(&shape[n - 1].author, &resolved, 1, &payloads[n - 1]);
                hashes[n - 1] = commit_hash(&resolved, &shape[n - 1].author, &payloads[n - 1]);
            }
            hashes[n - 1]
        }
        1 => hashes[u.choose_index(n)?],
        _ => pool[u.choose_index(pool.len())?],
    };
    let id = if fragment {
        let boundary = match u.int_in_range(0..=2)? {
            0 => naive_stops(t, &shape, depth(&head)),
            1 => {
                let mut b = BTreeSet::new();
                for _ in 0..u.int_in_range(0..=3)? {
                    b.insert(pool[u.choose_index(pool.len())?]);
                }
                b
            }
            _ => BTreeSet::new(),
        };
        ItemId::Fragment(FragmentRange { head, boundary })
    } else {
        ItemId::Commit(head)
    };
    let mut evidence = vec![];
    let mut signatures_valid = true;
    for i in 0..n {
        let other = (key_of[i].unwrap_or(0) + 1 + u.int_in_range(0..=1)?) % 3;
        match (u.int_in_range(0..=4)?, key_of[i]) {
            (0, Some(k)) => evidence.push(Evidence { author: author(&keys[k]), member: i, signature: sign(&keys[k], &hashes[i]) }),
            (0, None) | (1, _) => {}
            (2, _) => evidence.push(Evidence { author: author(&keys[other]), member: i, signature: sign(&keys[other], &hashes[i]) }),
            (3, _) => {
                signatures_valid = false;
                evidence.push(Evidence { author: shape[i].author, member: i, signature: sign(&keys[other], &hashes[i]) })
            }
            _ => {
                signatures_valid = false;
                evidence.push(Evidence { author: shape[i].author, member: i, signature: Signature::from(u.arbitrary::<[u8; 64]>()?) })
            }
        }
    }
    if u.ratio(1, 8)? {
        // Also sign a random member by a random key.
        let (i, k) = (u.choose_index(n)?, u.choose_index(3)?);
        evidence.push(Evidence { author: author(&keys[k]), member: i, signature: sign(&keys[k], &hashes[i]) });
    }
    let meta = BundleMeta { id, shape: FragmentShape { members: shape }, evidence };
    let mut bytes = wire::encode_header(&meta, &format());
    bytes.extend(chunk_body(payloads.iter().map(|p| p.as_slice())));
    let mut intact = true;
    if u.ratio(1, 8)? {
        let i = u.choose_index(bytes.len())?;
        bytes[i] ^= u.int_in_range(1..=255)?;
        intact = false;
    }
    Ok(Crafted { bytes, meta, hashes, signatures_valid, intact })
}

fn resolve(parents: &[ParentRef], hashes: &[CommitHash]) -> Vec<CommitHash> {
    parents
        .iter()
        .map(|p| match p {
            ParentRef::Member(j) => hashes[*j],
            ParentRef::External(h) => *h,
        })
        .collect()
}

/// Stops at depth >= level walking down from the external parents (template DAG).
fn naive_stops(t: &Templates, shape: &[MemberShape], level: usize) -> BTreeSet<CommitHash> {
    let mut stack: Vec<CommitHash> = shape
        .iter()
        .flat_map(|m| &m.parents)
        .filter_map(|p| match p {
            ParentRef::External(h) => Some(*h),
            _ => None,
        })
        .collect();
    let mut seen = BTreeSet::new();
    let mut stops = BTreeSet::new();
    while let Some(h) = stack.pop() {
        if !seen.insert(h) {
            continue;
        }
        if depth(&h) >= level.max(1) {
            stops.insert(h);
        } else if let Some(c) = t.commits.get(&h) {
            stack.extend(&c.parents);
        }
    }
    stops
}

/// Independent validity of a crafted bundle, excluding history placement.
/// Returns the reason it is invalid.
fn invalid_reason(c: &Crafted) -> Option<&'static str> {
    let meta = &c.meta;
    let n = meta.len();
    if !c.signatures_valid {
        return Some("bad signature");
    }
    let members: BTreeSet<_> = c.hashes.iter().collect();
    if members.len() != n {
        return Some("duplicate member");
    }
    // Inclusive in-bundle reachability.
    let reach = |from: usize| {
        let mut seen = vec![false; n];
        let mut stack = vec![from];
        while let Some(i) = stack.pop() {
            if std::mem::replace(&mut seen[i], true) {
                continue;
            }
            for p in &meta.shape.members[i].parents {
                if let ParentRef::Member(j) = p {
                    stack.push(*j);
                }
            }
        }
        seen
    };
    for i in 0..n {
        let covered = meta.evidence.iter().any(|e| {
            e.author == meta.shape.members[i].author && reach(e.member)[i]
        });
        if !covered {
            return Some("uncovered member");
        }
    }
    let head = meta.head();
    let Some(h) = c.hashes.iter().position(|m| *m == head) else {
        return Some("head not a member");
    };
    match &meta.id {
        ItemId::Commit(_) => {
            if n != 1 {
                return Some("commit with several members");
            }
        }
        ItemId::Fragment(range) => {
            let level = depth(&head);
            if level == 0 {
                return Some("shallow head");
            }
            if reach(h).iter().any(|r| !r) {
                return Some("member unreachable from head");
            }
            if range.boundary.iter().any(|b| depth(b) < level || members.contains(b)) {
                return Some("bad boundary");
            }
            if c.hashes.iter().any(|m| *m != head && depth(m) >= level) {
                return Some("deep member");
            }
            for p in meta.shape.members.iter().flat_map(|m| &m.parents) {
                if let ParentRef::External(x) = p {
                    if members.contains(x) {
                        return Some("external is a member");
                    }
                    if depth(x) >= level && !range.boundary.contains(x) {
                        return Some("deep external not a boundary");
                    }
                }
            }
        }
    }
    for p in meta.shape.members.iter().flat_map(|m| &m.parents) {
        if let ParentRef::External(x) = p
            && members.contains(x)
        {
            return Some("external is a member");
        }
    }
    None
}

/// Whether a structurally valid crafted bundle is admissible in `dag` now.
fn placeable(c: &Crafted, dag: &Dag) -> bool {
    let externals: BTreeSet<CommitHash> = c
        .meta
        .shape
        .members
        .iter()
        .flat_map(|m| &m.parents)
        .filter_map(|p| match p {
            ParentRef::External(h) => Some(*h),
            _ => None,
        })
        .collect();
    let mut deps = externals.clone();
    if let ItemId::Fragment(r) = &c.meta.id {
        deps.extend(&r.boundary);
    }
    if deps.iter().any(|d| !dag.index.get(d).is_some_and(|&i| dag.admitted[i])) {
        return false;
    }
    let ItemId::Fragment(range) = &c.meta.id else { return true };
    let below = dag.ancestry_of_hashes(&range.boundary.iter().copied().collect::<Vec<_>>());
    if c.hashes.iter().any(|m| dag.index.get(m).is_some_and(|&i| below.contains(i))) {
        return false;
    }
    if externals.iter().any(|x| !below.contains(dag.index[x])) {
        return false;
    }
    let level = depth(&range.head);
    let mut stops = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut stack: Vec<usize> = externals.iter().map(|x| dag.index[x]).collect();
    while let Some(i) = stack.pop() {
        if !seen.insert(i) {
            continue;
        }
        if depth(&dag.hashes[i]) >= level {
            stops.insert(dag.hashes[i]);
        } else {
            stack.extend(&dag.parents[i]);
        }
    }
    stops == range.boundary
}

fn after_done(history: &History, host: &Host, receipt: &Receipt, meta: Option<&BundleMeta>) {
    let dag = Dag::of(history);
    dag.check(history);
    for m in &receipt.members {
        assert!(history.is_admitted(&m.hash), "receipt member not admitted");
        assert_eq!(history.lookup(&m.hash).unwrap().payload, m.payload, "receipt payload ID differs");
    }
    if let Some(meta) = meta {
        assert_eq!(receipt.members.len(), meta.len(), "receipt member count");
        assert_eq!(&receipt.id, &meta.id, "receipt id");
    }
    if let ItemId::Fragment(range) = &receipt.id {
        dag.check_fragment(range);
        let (_, members) = dag.canonical_fragment(dag.index[&range.head]);
        let got: BTreeSet<_> = receipt.members.iter().map(|m| m.hash).collect();
        assert_eq!(got, members, "admitted fragment members differ from the canonical fragment");
    } else {
        assert_eq!(receipt.members.len(), 1);
        assert_eq!(receipt.members[0].hash, receipt.id.head(), "commit item admitted a different commit");
    }
    // Everything admitted is re-exportable into a fresh history.
    let items: Vec<Vec<u8>> = host.export_all(history).into_iter().map(|(_, b)| b).collect();
    let mut fresh_host = Host::default();
    let mut fresh = History::new();
    let waiting = receive_all(&mut fresh, &mut fresh_host, &items, &DriveConfig::default());
    assert!(waiting.is_empty(), "re-exported history does not admit: waiting on {waiting:?}");
    let fresh_dag = Dag::of(&fresh);
    assert_eq!(fresh_dag.admitted_set(), dag.admitted_set(), "re-export did not converge");
    for h in fresh_dag.admitted_set() {
        let a = &host.payloads[history.lookup(&h).unwrap().payload as usize];
        let b = &fresh_host.payloads[fresh.lookup(&h).unwrap().payload as usize];
        assert_eq!(a, b, "payload bytes differ after re-export");
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() < 9 {
        return;
    }
    let (cfg, rest) = data.split_at(9);
    let t = templates();
    let crafted_mode = cfg[0] % 4 == 3;
    let crafted = if crafted_mode {
        match craft(&mut Unstructured::new(rest), t) {
            Ok(c) => Some(c),
            Err(_) => return,
        }
    } else {
        None
    };
    let bytes = crafted.as_ref().map_or(rest, |c| c.bytes.as_slice()).to_vec();
    let stage = cfg[7] as usize % (t.stages.len() + 1);
    let drafts = cfg[8] & 2 != 0;
    let mut r = receiver(t, stage, drafts);
    let (epoch, len) = (r.history.epoch(), r.history.len());
    let dag_before = Dag::of(&r.history);
    // Storage keys are opaque: an item stored under an unrelated key must be
    // received exactly as under its content hash.
    let opaque_key = cfg[8] & 1 != 0;
    let item = if opaque_key {
        BlobHash::of_contents(&[b"opaque key".as_slice(), &bytes].concat())
    } else {
        BlobHash::of_contents(&bytes)
    };
    let preexisting = r.host.blobs.contains_key(&item);
    r.host.blobs.insert(item, bytes.clone());
    let options = options(cfg, crafted_mode);
    let cfg_drive = DriveConfig {
        read_limit: [1, 2, 3, 7, 64, 4096, usize::MAX][cfg[4] as usize % 7],
        decode_limit: [1, 2, 3, 7, 64, 4096, 65536][cfg[5] as usize % 7],
        max_steps: 64 * (bytes.len() + 4096),
    };
    let mut op = r.history.receive(options.clone());
    let outcome = r.host.run(&mut r.history, &mut op, item, &cfg_drive);
    if std::env::var_os("STOCKFOIL_FUZZ_TRACE").is_some() {
        let kind = match op.meta().map(|m| &m.id) {
            Some(ItemId::Commit(_)) => "commit",
            Some(ItemId::Fragment(_)) => "fragment",
            None => "no-header",
        };
        let what = match &outcome {
            Outcome::Done(_) => "Done".to_string(),
            Outcome::Waiting(_) => "Waiting".to_string(),
            Outcome::Failed(e) => format!("Failed {}", format!("{e:?}").split(['(', ' ', '{']).next().unwrap()),
            Outcome::Stuck => "Stuck".to_string(),
        };
        let valid = crafted.as_ref().map(|c| if !c.intact { "mutated" } else if invalid_reason(c).is_none() { "valid" } else { "invalid" });
        eprintln!("TRACE {} {kind} {what} {}", if crafted_mode { "crafted" } else { "raw" }, valid.unwrap_or("-"));
    }
    match &outcome {
        Outcome::Stuck => panic!("admission did not terminate"),
        Outcome::Done(receipt) => {
            assert!(options.format == format(), "admitted an item of another format");
            if let Some(c) = &crafted {
                if c.intact {
                    if let Some(why) = invalid_reason(c) {
                        panic!("admitted an invalid crafted bundle ({why}): {:?}", c.meta);
                    }
                    let got: Vec<_> = receipt.members.iter().map(|m| m.hash).collect();
                    assert_eq!(got, c.hashes, "admitted different commits than crafted");
                }
            }
            after_done(&r.history, &r.host, receipt, op.meta().map(|m| m.as_ref()));
            // Admission changed the history: do not cache.
            return;
        }
        Outcome::Failed(_) | Outcome::Waiting(_) => {
            assert_eq!(r.history.epoch(), epoch, "failed/waiting admission changed the epoch");
            assert_eq!(r.history.len(), len, "failed/waiting admission delivered commits");
            if let Some(c) = &crafted {
                let default_shape = options.read_size > 0
                    && options.decode_capacity > 0
                    && options.format == format();
                if c.intact
                    && default_shape
                    && invalid_reason(c).is_none()
                    && placeable(c, &dag_before)
                {
                    panic!("valid, placeable crafted bundle not admitted: {outcome:?}\n{:?}", c.meta);
                }
            }
        }
    }
    if !preexisting {
        r.host.blobs.remove(&item);
    }
    CACHE.with(|c| c.borrow_mut().insert((stage, drafts), r));
});
