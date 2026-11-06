//! In-memory byte handling and the ledger boundary.
mod support;
use stockfoil::*;
use support::*;

#[test]
fn commit_identity_parents_signatures_and_drafts() {
    let (alice, bob) = (key(1), key(2));
    let mut doc = document_for(&alice);
    let root = doc.commit(&b"draft"[..], None).unwrap();
    assert_eq!(root.hash, commit_hash(&[], &author(&alice), b"draft"));
    let child = doc
        .commit(&b"child"[..], Some(vec![root.hash, root.hash]))
        .unwrap();
    assert_eq!(child.parents, [root.hash, root.hash]);
    assert_eq!(
        child.hash,
        commit_hash(&child.parents, &author(&alice), b"child")
    );
    assert_eq!(doc.drafts().count(), 2);
    assert!(doc.export(&ExportCursor::new()).is_empty());
    assert!(matches!(
        doc.attach_signature(&root, sign(&bob, &root.hash)),
        Err(Error::InvalidSignature { .. })
    ));
    assert!(
        doc.attach_signature(&child, sign(&alice, &child.hash))
            .unwrap()
            .admitted
            .is_empty()
    );
    let signed = doc
        .attach_signature(&root, sign(&alice, &root.hash))
        .unwrap();
    assert_eq!(signed.admitted, [root.hash, child.hash]);
    assert!(!doc.has_drafts());
    assert!(
        doc.attach_signature(&root, sign(&alice, &root.hash))
            .unwrap()
            .admitted
            .is_empty()
    );
    assert_eq!(doc.payload(&child.hash).unwrap().as_ref(), b"child");
    let unknown = CommitHash::from([7; 32]);
    assert_eq!(
        doc.commit(&b"x"[..], Some(vec![unknown])).unwrap_err(),
        Error::MissingParent(unknown)
    );
}

fn chain_items() -> (Vec<CommitHash>, Vec<Vec<u8>>) {
    let alice = key(1);
    let mut doc = document();
    let a = signed(&mut doc, &alice, vec![], b"1");
    let b = signed(&mut doc, &alice, vec![a], b"2");
    let c = signed(&mut doc, &alice, vec![b], b"3");
    let export = doc.export(&ExportCursor::new());
    let items: Vec<_> = (0..export.len()).map(|i| export.item(i).unwrap()).collect();
    let bytes = [a, b, c]
        .iter()
        .map(|hash| {
            items
                .iter()
                .find(|(item, _)| item.id.head() == *hash)
                .unwrap()
                .1
                .clone()
        })
        .collect();
    (vec![a, b, c], bytes)
}

#[test]
fn out_of_order_receipt_releases_all_items_and_duplicate_binding_is_stable() {
    let (hashes, items) = chain_items();
    let mut doc = document_for(&key(2));
    assert_eq!(
        doc.receive(&items[2]).unwrap(),
        Received::Waiting(hashes[1])
    );
    assert_eq!(
        doc.receive(&items[1]).unwrap(),
        Received::Waiting(hashes[0])
    );
    assert_eq!(doc.ledger().parked(), 2);
    let Received::Admitted {
        items: receipts,
        delivered,
        failed,
    } = doc.receive(&items[0]).unwrap()
    else {
        panic!()
    };
    assert_eq!(receipts.len(), 3);
    assert_eq!(delivered, hashes);
    assert!(failed.is_empty());
    assert_eq!(doc.ledger().parked(), 0);
    let before = doc.ledger().payload_id(&hashes[1]);
    let Received::Admitted { delivered, .. } = doc.receive(&items[1]).unwrap() else {
        panic!()
    };
    assert!(delivered.is_empty());
    assert_eq!(doc.ledger().payload_id(&hashes[1]), before);
    assert_eq!(doc.payload(&hashes[2]).unwrap().as_ref(), b"3");
}

#[test]
fn eviction_and_zero_capacity_are_observable_and_items_can_be_requested_again() {
    let (hashes, items) = chain_items();
    let mut doc = document().with_max_parked(1);
    doc.receive(&items[2]).unwrap();
    assert_eq!(
        doc.receive(&items[1]).unwrap(),
        Received::Evicted {
            waiting: hashes[0],
            evicted: ItemId::Commit(hashes[2]),
        }
    );
    doc.receive(&items[0]).unwrap();
    assert!(!doc.ledger().contains(&hashes[2]));
    doc.receive(&items[2]).unwrap();
    assert_eq!(doc.ledger().len(), 3);
    let mut zero = document().with_max_parked(0);
    assert!(
        matches!(zero.receive(&items[2]).unwrap(), Received::Evicted { evicted, .. } if evicted == ItemId::Commit(hashes[2]))
    );
    assert_eq!(zero.ledger().parked(), 0);
}

#[test]
fn duplicate_parked_copies_share_staging_without_losing_the_survivor() {
    let (hashes, items) = chain_items();
    let mut doc = document().with_max_parked(1);
    doc.receive(&items[1]).unwrap();
    assert!(
        matches!(doc.receive(&items[1]).unwrap(), Received::Evicted { evicted, .. } if evicted == ItemId::Commit(hashes[1]))
    );
    doc.receive(&items[0]).unwrap();
    assert!(doc.ledger().is_admitted(&hashes[1]));
    assert_eq!(doc.payload(&hashes[1]).unwrap().as_ref(), b"2");
}

#[test]
fn an_owned_export_plan_does_not_hold_a_document_borrow_during_io() {
    let mut doc = document();
    let first = doc.commit(&b"one"[..], None).unwrap();
    doc.sign_drafts(&key(1)).unwrap();
    let mut plan = doc.export(&ExportCursor::new()).into_plan();
    let first_item = doc.export_item(&plan, 0).unwrap();
    let second = doc.commit(&b"two"[..], None).unwrap();
    doc.sign_drafts(&key(1)).unwrap();
    assert_eq!(plan.len(), 1);
    assert_eq!(doc.export_item(&plan, 0).unwrap(), first_item);
    plan.acknowledge(0).unwrap();
    let export = doc.export(&plan.cursor());
    assert_eq!(export.len(), 1);
    assert_eq!(export.item(0).unwrap().0.head(), second.hash);
    assert_ne!(first.hash, second.hash);
}

#[test]
fn caller_buffer_options_preserve_the_trusted_format() {
    let ledger = Ledger::new(format(), author(&key(1)));
    let mut options = ledger.receive_options();
    options.read_size = 7;
    options.decode_capacity = 11;
    options.quantum = 1;
    let mut op = ledger.begin_receive_with(options.clone()).unwrap();
    assert!(matches!(
        op.next(),
        Next::Request(_, Request::Read { max: 7, .. })
    ));
    options.format = FormatId::new("other/format").unwrap();
    assert!(matches!(
        ledger.begin_receive_with(options),
        Err(Error::WrongFormat)
    ));
}

#[test]
fn byte_verification_cannot_be_bypassed_and_bad_items_do_not_publish() {
    let mut ledger = Ledger::new(format(), author(&key(1)));
    let op = ledger.begin_receive();
    assert!(matches!(
        ledger.admit(op, &mut |_: &ItemId, _: &[CommitHash]| vec![]),
        Err(Error::Protocol(_))
    ));
    let (_, items) = chain_items();
    let mut doc = document();
    let mut bad = items[0].clone();
    *bad.last_mut().unwrap() ^= 1;
    assert!(doc.receive(&bad).is_err());
    assert!(doc.history().is_empty());
    doc.receive(&items[0]).unwrap();
    assert_eq!(doc.history().len(), 1);
}
