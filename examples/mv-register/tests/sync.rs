//! In-process synchronization: per-peer export cursors, out-of-order
//! delivery, and convergence.
use std::collections::HashMap;
use stockfoil::{ExportCursor, Heads, WritePermission};
use stockfoil_mv_register::{MvRegisterMap, Received};

#[test]
fn items_received_before_their_dependencies_wait_then_admit() {
    let mut writer = MvRegisterMap::with_key(ed25519_dalek::SigningKey::from_bytes(&[1; 32]));
    writer.set("title", "1").unwrap();
    writer.set("title", "2").unwrap();
    writer.set("title", "3").unwrap();
    let (_, items) = writer.export_items(&ExportCursor::new()).unwrap();
    assert_eq!(items.len(), 3);

    let mut reader = MvRegisterMap::new();
    reader
        .set_write_permissions(writer.write_permissions().clone())
        .unwrap();
    // Deliver in every order: each item admits once its parent has.
    let mut outcomes = vec![];
    for item in items.iter().rev() {
        outcomes.push(reader.receive(item).unwrap());
    }
    let admitted = outcomes
        .iter()
        .filter(|o| matches!(o, Received::Admitted { .. }))
        .count();
    assert!(admitted >= 1);
    assert_eq!(reader.waiting(), 0);
    assert_eq!(reader.values(), writer.values());
    assert_eq!(
        reader
            .document()
            .history()
            .heads(Heads::Admitted)
            .collect::<Vec<_>>(),
        writer
            .document()
            .history()
            .heads(Heads::Admitted)
            .collect::<Vec<_>>()
    );
    // Receiving again delivers nothing new.
    for item in &items {
        assert_eq!(reader.receive(item).unwrap(), Received::Admitted { new: 0 });
    }
}

#[test]
fn cursors_send_each_item_once_per_peer() {
    let mut alice = MvRegisterMap::with_key(ed25519_dalek::SigningKey::from_bytes(&[1; 32]));
    let mut bob = MvRegisterMap::new();
    alice.set("a", "1").unwrap();
    alice.set("b", "2").unwrap();
    assert_eq!(alice.sync_to(&mut bob).unwrap(), 2);
    assert_eq!(alice.sync_to(&mut bob).unwrap(), 0);
    // Bob need not echo what he just received from Alice.
    assert_eq!(bob.sync_to(&mut alice).unwrap(), 0);
    alice.set("a", "3").unwrap();
    assert_eq!(alice.sync_to(&mut bob).unwrap(), 1);
    // A different peer has its own cursor.
    let mut carol = MvRegisterMap::new();
    assert_eq!(bob.sync_to(&mut carol).unwrap(), 3);
}

/// A small deterministic generator, so failures reproduce.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) % n as u64) as usize
    }
}

#[test]
fn random_edits_and_syncs_converge() {
    let mut rng = Lcg(7);
    let mut docs: Vec<MvRegisterMap> = (0..3).map(|_| MvRegisterMap::new()).collect();
    let everyone: HashMap<_, _> = docs
        .iter()
        .map(|d| (d.author(), WritePermission::Unbounded))
        .collect();
    for doc in &mut docs {
        doc.set_write_permissions(everyone.clone()).unwrap();
    }
    for step in 0..200 {
        let i = rng.next(3);
        if rng.next(3) == 0 {
            let j = (i + 1 + rng.next(2)) % 3;
            let (from, to) = pair(&mut docs, i, j);
            from.sync_to(to).unwrap();
        } else {
            let key = ["title", "body", "tag"][rng.next(3)];
            if rng.next(4) == 0 {
                docs[i].prepare_set(key, &format!("draft {step}")).unwrap();
            } else {
                docs[i].set(key, &format!("{i}:{step}")).unwrap();
            }
        }
    }
    for doc in &mut docs {
        doc.publish().unwrap();
    }
    for _ in 0..2 {
        for i in 0..3 {
            for j in 0..3 {
                if i != j {
                    let (from, to) = pair(&mut docs, i, j);
                    from.sync_to(to).unwrap();
                }
            }
        }
    }
    for doc in &docs[1..] {
        assert_eq!(doc.values(), docs[0].values());
        assert_eq!(
            doc.document().history().len(),
            docs[0].document().history().len()
        );
    }
    assert!(
        docs[0].values().values().any(|v| v.len() > 1),
        "expected a conflict"
    );
    for i in 0..3 {
        for j in 0..3 {
            if i != j {
                let (from, to) = pair(&mut docs, i, j);
                assert_eq!(from.sync_to(to).unwrap(), 0);
            }
        }
    }
}

fn pair(
    docs: &mut [MvRegisterMap],
    i: usize,
    j: usize,
) -> (&mut MvRegisterMap, &mut MvRegisterMap) {
    assert_ne!(i, j);
    if i < j {
        let (a, b) = docs.split_at_mut(j);
        (&mut a[i], &mut b[0])
    } else {
        let (a, b) = docs.split_at_mut(i);
        (&mut b[0], &mut a[j])
    }
}

const TREE: sedimentree_core::id::SedimentreeId =
    sedimentree_core::id::SedimentreeId::new([42; 32]);

/// A value whose assignment, committed next by `doc`, has depth >= 1 (so
/// signing it assembles a fragment).
fn checkpoint_value(doc: &MvRegisterMap, key: &str) -> String {
    let history = doc.document().history();
    let heads: Vec<_> = history.heads(Heads::Delivered).collect();
    (0u64..)
        .map(|i| format!("checkpoint {i}"))
        .find(|value| {
            let mut builder = history.begin_commit(doc.author(), heads.clone()).unwrap();
            builder.update(&stockfoil_mv_register::Assignment::new(key, value.as_str()).encode());
            builder.finish().hash.as_bytes()[0] == 0
        })
        .unwrap()
}

#[tokio::test]
async fn subduction_tokio_load_and_watch_syncs_initial_and_live_edits() {
    use stockfoil_mv_register::SubductionReplica;

    let mut alice = SubductionReplica::new(TREE).await.unwrap();
    let mut bob = SubductionReplica::new(TREE).await.unwrap();
    let mut carol = SubductionReplica::new(TREE).await.unwrap();
    let everyone = HashMap::from([
        (alice.author(), WritePermission::Unbounded),
        (bob.author(), WritePermission::Unbounded),
        (carol.author(), WritePermission::Unbounded),
    ]);
    alice.set_write_permissions(everyone.clone()).unwrap();
    bob.set_write_permissions(everyone.clone()).unwrap();
    carol.set_write_permissions(everyone).unwrap();

    // Alice writes several commits plus a depth-1 fragment head before connecting.
    for i in 0..5 {
        alice.set("count", &format!("{i}")).await.unwrap();
    }
    let cp = checkpoint_value(alice.doc(), "count");
    alice.set("count", &cp).await.unwrap();
    bob.set("bob_key", "offline").await.unwrap();

    // Connecting Alice and Bob triggers `load_and_watch`'s automatic initial sync.
    let (ab_left, ab_right) = alice.connect(&mut bob).await.unwrap();
    assert_eq!(alice.values(), bob.values());
    assert_eq!(bob.get("count").iter().next().unwrap(), &cp);

    // Concurrent writes after connection converge after a sync round.
    alice.set("title", "from alice").await.unwrap();
    bob.set("title", "from bob").await.unwrap();
    alice.sync_with(&mut bob).await.unwrap();
    assert_eq!(alice.values(), bob.values());
    assert_eq!(alice.get("title").len(), 2);

    // Carol connects to Bob and receives both Alice's fragment and Bob's writes.
    let (bc_left, bc_right) = bob.connect(&mut carol).await.unwrap();
    assert_eq!(carol.values(), bob.values());

    drop((ab_left, ab_right, bc_left, bc_right));
    assert!(!alice.close().await.forced);
    assert!(!bob.close().await.forced);
    assert!(!carol.close().await.forced);
}

#[tokio::test]
async fn subduction_tokio_replays_existing_local_storage_on_open() {
    use stockfoil_mv_register::SubductionReplica;
    use subduction_core::storage::memory::MemoryStorage;

    let shared_storage = MemoryStorage::default();
    let mut writer = MvRegisterMap::new();
    let author = writer.author();
    writer.set("greeting", "persisted in subduction").unwrap();

    let first = SubductionReplica::with_storage(TREE, writer, shared_storage.clone())
        .await
        .unwrap();
    assert!(!first.close().await.forced);

    // Opening a fresh replica against the same Subduction storage replays the
    // stored commit through `load_and_watch` before `StorageChecked`.
    let mut reader_doc = MvRegisterMap::new();
    reader_doc
        .set_write_permissions(HashMap::from([(author, WritePermission::Unbounded)]))
        .unwrap();
    let reopened = SubductionReplica::with_storage(TREE, reader_doc, shared_storage)
        .await
        .unwrap();
    assert_eq!(
        reopened.get("greeting"),
        std::collections::BTreeSet::from(["persisted in subduction".to_owned()])
    );
    assert!(!reopened.close().await.forced);
}
