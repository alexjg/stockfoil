//! Timing benchmarks for large histories. Run with
//! `cargo bench --bench history` (optionally `-- 1000000` for the node count).
//!
//! The history models a long-lived document: a few authors whose device keys
//! rotate, mostly sequential edits with occasional two-way concurrency, and a
//! checkpoint-depth commit roughly every 256 commits.
use std::collections::HashMap;
use std::time::{Duration, Instant};
use stockfoil::PayloadId;
use stockfoil::*;

fn author(i: u32) -> Author {
    let mut bytes = [0u8; 32];
    bytes[..4].copy_from_slice(&i.to_le_bytes());
    Author::try_from(&bytes[..]).unwrap()
}

fn time<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    report(label, start.elapsed());
    out
}

fn report(label: &str, d: Duration) {
    println!("{label:<58} {:>10.3} ms", d.as_secs_f64() * 1e3);
}

/// Append `n` drafts. Returns the authors used.
fn grow(history: &mut History, n: usize, seed: u64) -> Vec<Author> {
    let mut authors = Vec::new();
    let mut tips: Vec<CommitHash> = history.heads(Heads::Delivered).collect();
    for i in 0..n {
        // Keys rotate every 50k commits; three devices are active at once.
        let generation = ((i as u64 + seed) / 50_000) as u32;
        let a = author(generation * 3 + (i % 3) as u32);
        if !authors.contains(&a) {
            authors.push(a);
        }
        let parents = if i % 97 == 0 && tips.len() > 1 {
            // Merge the concurrent branches.
            std::mem::take(&mut tips)
        } else if i % 89 == 0 {
            // Fork: keep the old tip as a concurrent head.
            tips.last().copied().into_iter().collect()
        } else {
            tips.pop().into_iter().collect()
        };
        let mut b = history.begin_commit(a, parents).unwrap();
        b.update(&(i as u64 + seed * 1_000_000_007).to_le_bytes());
        let p = b.finish();
        tips.push(p.hash);
        history.publish_commit(p, i as PayloadId).unwrap();
    }
    authors
}

fn main() {
    let n: usize = std::env::args()
        .filter_map(|a| a.parse().ok())
        .next()
        .unwrap_or(1_000_000);
    println!("history of {n} commits");
    let mut history = History::new();
    let authors = time(&format!("build {n} local commits"), || {
        grow(&mut history, n, 0)
    });
    let permissions: WritePermissions = authors
        .iter()
        .map(|a| (*a, WritePermission::Unbounded))
        .collect();
    println!(
        "{} authors, {} heads",
        authors.len(),
        history.heads(Heads::Delivered).count()
    );

    // Benchmark canonical traversal through the public causal iterator. Raw
    // change cursors are an implementation detail of the planner below.
    let mut cold = time("plan cold causal traversal (no record expansion)", || {
        history.commits_since(&[]).unwrap()
    });
    assert_eq!(cold.len(), n);
    let first = time("first cold prefix (up to 4096 commits)", || {
        cold.by_ref().take(4096).count()
    });
    assert_eq!(first, n.min(4096));
    drop(cold);
    let count = time("cold causal traversal (all commits)", || {
        history.commits_since(&[]).unwrap().count()
    });
    assert_eq!(count, n);

    let before: Vec<_> = history.heads(Heads::Delivered).collect();
    let count = time("no-op causal difference", || {
        history.commits_since(&before).unwrap().count()
    });
    assert_eq!(count, 0);

    grow(&mut history, 10, 7);
    let count = time("small causal difference (10 new commits)", || {
        history.commits_since(&before).unwrap().count()
    });
    assert_eq!(count, 10);

    let heads: Vec<_> = history.heads(Heads::Delivered).collect();
    let old = history.traverse().nth(n / 2).unwrap();
    time("1000 is_ancestor queries (old vs head)", || {
        for _ in 0..1000 {
            assert!(history.is_ancestor(&old, &heads[0]));
        }
    });

    let at = time("capture ancestry at current heads", || {
        history.ancestry(&heads, &permissions).unwrap()
    });
    time("1000 captured ancestry membership queries", || {
        for _ in 0..1000 {
            assert!(at.contains(&history, &old).unwrap());
        }
    });
    let past = history.ancestry(&[old], &permissions).unwrap();
    let difference = time("plan causal difference from midpoint", || {
        at.difference(&past, &history).unwrap()
    });
    let remaining = difference.len();
    let count = time("enumerate causal difference", || difference.count());
    assert_eq!(count, remaining);
    println!("    {count} commits");

    drop(history);
    planner_bench(n);
    signing_and_admission();
}

/// The actual ledger/planner path: no payload storage, only caller-assigned IDs.
fn planner_bench(n: usize) {
    let mut ledger = Ledger::new(FormatId::new("bench/planner-v1").unwrap(), author(1));
    time("build ledger for planner benchmark", || {
        for i in 0..n {
            let mut builder = ledger.begin_commit(None).unwrap();
            builder.update(&(i as u64).to_le_bytes());
            ledger.commit(builder.finish(), i as PayloadId).unwrap();
        }
    });
    let mut m = Materialization::new(Follow::Working, true);
    let mut count = 0;
    time("cold planner (plan + complete batches of 4096)", || {
        while let Some(batch) = m.plan(&ledger, 4096).unwrap() {
            count += batch.len();
            assert!(
                m.complete(&ledger, batch, Applied::default())
                    .unwrap()
                    .is_none()
            );
        }
    });
    assert_eq!(count, n);
    for i in 0..10 {
        let mut builder = ledger.begin_commit(None).unwrap();
        builder.update(&[0xff, i]);
        ledger
            .commit(builder.finish(), (n + i as usize) as PayloadId)
            .unwrap();
    }
    let mut count = 0;
    time("incremental planner (10 commits)", || {
        while let Some(batch) = m.plan(&ledger, 4096).unwrap() {
            count += batch.len();
            m.complete(&ledger, batch, Applied::default()).unwrap();
        }
    });
    assert_eq!(count, 10);
    if n > 4096 {
        let mut first = Materialization::new(Follow::Working, true);
        let batch = first.plan(&ledger, 4096).unwrap().unwrap();
        first.complete(&ledger, batch, Applied::default()).unwrap();
        let bytes = first.checkpoint().to_bytes();
        time("resume planner after 4096 processed records", || {
            let mut restored =
                Materialization::resume(Checkpoint::from_bytes(&bytes).unwrap(), true);
            let batch = restored.plan(&ledger, 1).unwrap().unwrap();
            restored
                .complete(&ledger, batch, Applied::default())
                .unwrap();
        });
    }

    ledger.set_permissions(HashMap::from([(ledger.author(), WritePermission::Denied)]));
    let mut retracted = 0;
    time("planner revocation (all local commits)", || {
        while let Some(batch) = m.plan(&ledger, 4096).unwrap() {
            retracted += batch
                .events()
                .iter()
                .filter(|e| matches!(e, Event::Retract { .. }))
                .count();
            m.complete(&ledger, batch, Applied::default()).unwrap();
        }
    });
    assert_eq!(retracted, n + 10);
}

/// Signed history: local signing with fragment assembly, export, and
/// admission through the native adapter with an in-memory store.
fn signing_and_admission() {
    use support::*;
    let n = 20_000;
    let alice = key(5);
    let mut a = document_for(&alice);
    let start = Instant::now();
    let mut tip = vec![];
    for i in 0..n {
        tip = vec![signed(&mut a, &alice, tip, &(i as u64).to_le_bytes())];
    }
    report(
        &format!("commit + sign {n} commits (incl. fragment assembly)"),
        start.elapsed(),
    );
    let plan = time("export plan", || a.export(&ExportCursor::new()));
    println!("    {} items cover {n} commits", plan.len());
    let mut items = Vec::new();
    time("write every planned item", || {
        for i in 0..plan.len() {
            let bytes = plan.item(i).map(|(_, bytes)| bytes).unwrap();
            items.push(bytes);
        }
    });
    let mut b = document();
    time("receive every item (verify + admit)", || {
        for item in &items {
            b.receive(item.as_slice()).unwrap();
        }
    });
    assert_eq!(b.history().len(), n);
    let _ = HashMap::<u8, u8>::new();
}

#[path = "../tests/support/mod.rs"]
mod support;
