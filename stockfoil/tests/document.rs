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

#[test]
fn preparation_captures_delivered_heads_and_distinguishes_an_explicit_root() {
    let mut doc = document();
    let a = doc.commit(&b"a"[..], Some(vec![])).unwrap().hash;
    let b = doc.commit(&b"b"[..], Some(vec![])).unwrap().hash;
    let c = doc.commit(&b"c"[..], Some(vec![a])).unwrap().hash;
    let mut expected = vec![b, c];
    expected.sort_unstable();
    let who = doc.author();
    let mut captured = doc.ledger().begin_commit(None).unwrap();
    let mut explicit = doc.ledger().begin_commit(Some(vec![])).unwrap();
    // Neither already-created builder is retargeted by a later delivery.
    doc.commit(&b"later"[..], Some(vec![])).unwrap();
    captured.update(b"merge");
    explicit.update(b"root");
    let captured = captured.finish();
    assert_eq!(captured.hash, commit_hash(&expected, &who, b"merge"));
    let captured = doc.ledger_mut().commit(captured, 99).unwrap();
    assert_eq!(captured.parents, expected);
    let explicit = explicit.finish();
    assert_eq!(explicit.hash, commit_hash(&[], &who, b"root"));
    assert!(
        doc.ledger_mut()
            .commit(explicit, 100)
            .unwrap()
            .parents
            .is_empty()
    );
}

#[test]
fn preparation_keeps_parent_order_and_repetitions_across_clone_and_chunking() {
    let mut doc = document();
    let a = doc.commit(&b"a"[..], Some(vec![])).unwrap().hash;
    let b = doc.commit(&b"b"[..], Some(vec![])).unwrap().hash;
    let parents = vec![b, a, b];
    let mut original = doc.ledger().begin_commit(Some(parents.clone())).unwrap();
    original.update(b"pre");
    let mut cloned = original.clone();
    original.update(b"fix-payload");
    cloned.update(b"fix-");
    cloned.update(b"pay");
    cloned.update(b"load");
    let first = original.finish();
    let second = cloned.finish();
    assert_eq!(first, second);
    assert_eq!(
        first.hash,
        commit_hash(&parents, &doc.author(), b"prefix-payload")
    );
    let draft = doc.ledger_mut().commit(first, 99).unwrap();
    assert_eq!(draft.parents, parents);
    assert_eq!(draft.author, doc.author());
}

#[test]
fn preparation_reports_first_missing_parent_before_building_and_preserves_history() {
    let mut doc = document();
    let known = doc.commit(&b"known"[..], None).unwrap().hash;
    let a = CommitHash::from([71; 32]);
    let b = CommitHash::from([72; 32]);
    let epoch = doc.history().epoch();
    let heads = doc.heads(Heads::Delivered);
    for (parents, first) in [(vec![known, a, b], a), (vec![b, known, a], b)] {
        assert_eq!(
            doc.ledger()
                .begin_commit(Some(parents.clone()))
                .unwrap_err(),
            Error::MissingParent(first)
        );
        assert_eq!(
            doc.history()
                .begin_commit(doc.author(), parents)
                .unwrap_err(),
            Error::MissingParent(first)
        );
    }
    assert_eq!(doc.history().epoch(), epoch);
    assert_eq!(doc.heads(Heads::Delivered), heads);
}

#[test]
fn public_commit_hashes_the_actual_native_input_conversion() {
    struct Converted;
    impl From<Converted> for Bytes {
        fn from(_: Converted) -> Self {
            Bytes::from(&b"converted canonical bytes"[..])
        }
    }
    let mut doc = document();
    let draft = doc.commit(Converted, Some(vec![])).unwrap();
    assert_eq!(
        draft.hash,
        commit_hash(&[], &doc.author(), b"converted canonical bytes")
    );
    assert_eq!(
        doc.payload(&draft.hash).unwrap().as_ref(),
        b"converted canonical bytes"
    );
}
