//! Timing benchmarks for large histories. Run with
//! `cargo bench --bench history` (optionally `-- 1000000` for the node count).
//!
//! The history models a long-lived document: a few authors whose device keys
//! rotate, mostly sequential edits with occasional two-way concurrency, and a
//! checkpoint-depth commit roughly every 256 commits.
use std::collections::HashMap;
use std::time::{Duration, Instant};
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

fn drain(
    history: &History,
    cursor: &MaterializationCursor,
    target: &ViewPoint,
) -> (usize, MaterializationCursor) {
    let mut changes = history.changes(cursor, target).unwrap();
    let mut out = Vec::with_capacity(4096);
    let mut total = 0;
    loop {
        out.clear();
        let batch = history.next_changes(&mut changes, &mut out, 4096).unwrap();
        total += out.len();
        if let Batch::Complete(next) = batch {
            return (total, next);
        }
    }
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

    let target = history.view_point(&permissions);
    let (count, cursor) = time("cold materialization (all records)", || {
        drain(&history, &MaterializationCursor::new(), &target)
    });
    assert_eq!(count, n);

    let (count, cursor) = time("no-op update", || drain(&history, &cursor, &target));
    assert_eq!(count, 0);

    grow(&mut history, 10, 7);
    let target = history.view_point(&permissions);
    let (count, cursor) = time("small update (10 new commits)", || {
        drain(&history, &cursor, &target)
    });
    assert_eq!(count, 10);

    let mut revoked = permissions.clone();
    let victim = authors[authors.len() - 1];
    let frontier: Vec<_> = history.heads(Heads::Delivered).take(1).collect();
    revoked.insert(victim, WritePermission::Bounded(frontier));
    let target = history.view_point(&revoked);
    let (count, cursor) = time("bound one author at the current heads", || {
        drain(&history, &cursor, &target)
    });
    println!("    {count} records");
    revoked.insert(victim, WritePermission::Denied);
    let target = history.view_point(&revoked);
    let (count, _) = time("revoke that author entirely", || {
        drain(&history, &cursor, &target)
    });
    println!("    {count} records");

    let heads: Vec<_> = history.heads(Heads::Delivered).collect();
    let old = history.traverse().nth(n / 2).unwrap();
    time("1000 is_ancestor queries (old vs head)", || {
        for _ in 0..1000 {
            assert!(history.is_ancestor(&old, &heads[0]));
        }
    });

    signing_and_admission();
}

/// Signed history: local signing with fragment assembly, export, and
/// admission through the native adapter with an in-memory store.
fn signing_and_admission() {
    use support::*;
    let n = 20_000;
    let alice = key(5);
    let mut a = graph();
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
            let mut bytes = Vec::new();
            a.write_item(&plan, i, &mut bytes).unwrap();
            items.push(bytes);
        }
    });
    let mut b = graph();
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
