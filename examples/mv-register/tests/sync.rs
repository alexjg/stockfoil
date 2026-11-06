//! In-process synchronization: per-peer export cursors, out-of-order
//! delivery, and convergence.
use stockfoil::{ExportCursor, Heads, WritePermission, WritePermissions};
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
    let everyone: WritePermissions = docs
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
