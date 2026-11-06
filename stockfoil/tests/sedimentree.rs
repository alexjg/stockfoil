#![cfg(feature = "sedimentree")]
mod support;

use sedimentree_core::{
    blob::Blob,
    fragment::{Fragment, InvalidFragment, MAX_BOUNDARIES, MAX_CHECKPOINTS},
    id::SedimentreeId,
    loose_commit::{InvalidLooseCommit, MAX_PARENTS, id::CommitId},
};
use std::{collections::BTreeSet, error::Error as _, sync::Arc};
use stockfoil::sedimentree::{Error, Metadata, describe};
use stockfoil::*;
use support::*;

const TREE: SedimentreeId = SedimentreeId::new([42; 32]);

fn id(hash: CommitHash) -> CommitId {
    CommitId::new(*hash.as_bytes())
}

fn exported(doc: &TestDocument, head: CommitHash) -> (ExportedItem, Vec<u8>) {
    let plan = doc.ledger().export(&ExportCursor::new());
    let index = (0..plan.len())
        .find(|&i| plan.id(i).unwrap().head() == head)
        .unwrap();
    doc.export_item(&plan, index).unwrap()
}

fn standalone() -> (ExportedItem, Vec<u8>) {
    let mut doc = document();
    let head = signed(&mut doc, &key(1), vec![], b"root");
    let item = exported(&doc, head);
    assert!(matches!(item.0.id, ItemId::Commit(_)));
    item
}

#[test]
fn standalone_preserves_identity_and_deduplicates_parents() {
    let mut doc = document();
    let root = signed(&mut doc, &key(1), vec![], b"root");
    let child = signed(&mut doc, &key(1), vec![root, root], b"child");
    let (item, bytes) = exported(&doc, child);
    let blob = Blob::new(bytes.clone());
    let Metadata::Commit(commit) = describe(TREE, &item, blob.meta()).unwrap() else {
        panic!("expected standalone commit");
    };
    assert_eq!(commit.head(), id(child));
    assert_eq!(commit.parents(), &BTreeSet::from([id(root)]));
    assert_eq!(commit.sedimentree_id(), TREE);
    assert_eq!(*commit.blob_meta(), blob.meta());
    // Conversion must not normalize the Stockfoil parent list or rewrite bytes.
    assert_eq!(item.meta.members[0].parents.len(), 2);
    let mut other = document();
    other.receive(&exported(&doc, root).1).unwrap();
    other.receive(&bytes).unwrap();
    assert_eq!(
        other.history().parents(&child).unwrap().collect::<Vec<_>>(),
        [root, root]
    );
}

#[test]
fn fragments_preserve_boundaries_and_interior_checkpoints() {
    let mut doc = document();
    let k = key(1);
    let root = signed(&mut doc, &k, vec![], b"root");
    let shallow = signed(
        &mut doc,
        &k,
        vec![root],
        &payload_at_depth(&k, &[root], 1, "shallow"),
    );
    let deep = signed(
        &mut doc,
        &k,
        vec![shallow],
        &payload_at_depth(&k, &[shallow], 2, "deep"),
    );
    let (item, bytes) = exported(&doc, deep);
    let blob = Blob::new(bytes);
    assert_eq!(item.checkpoints, [shallow]);
    assert_eq!(
        describe(TREE, &item, blob.meta()).unwrap(),
        Metadata::Fragment(
            Fragment::new(TREE, id(deep), BTreeSet::new(), &[id(shallow)], blob.meta()).unwrap()
        )
    );

    let next = signed(
        &mut doc,
        &k,
        vec![deep],
        &payload_at_depth(&k, &[deep], 2, "next"),
    );
    let (item, bytes) = exported(&doc, next);
    let blob = Blob::new(bytes);
    let Metadata::Fragment(fragment) = describe(TREE, &item, blob.meta()).unwrap() else {
        panic!("expected fragment");
    };
    assert_eq!(fragment.head(), id(next));
    assert_eq!(fragment.boundary(), &BTreeSet::from([id(deep)]));
    assert!(fragment.checkpoints().is_empty());
    assert_eq!(fragment.sedimentree_id(), TREE);
    assert_eq!(fragment.summary().blob_meta(), blob.meta());
}

#[test]
fn conversion_uses_caller_blob_metadata_and_needs_no_live_document() {
    let (item, bytes) = standalone(); // The document has already been dropped.
    let plain = Blob::new(bytes.clone());
    let sealed = Blob::new(bytes.into_iter().map(|b| b ^ 0xa5).collect());
    assert_ne!(plain.meta(), sealed.meta());
    let other_tree = SedimentreeId::new([9; 32]);
    let Metadata::Commit(commit) = describe(other_tree, &item, sealed.meta()).unwrap() else {
        panic!("expected standalone commit");
    };
    assert_eq!(commit.sedimentree_id(), other_tree);
    assert_eq!(*commit.blob_meta(), sealed.meta());
    assert_eq!(commit.head(), id(item.head()));
}

#[test]
fn ledger_exports_work_without_encoding_a_body_or_acknowledging_a_plan() {
    let mut doc = document();
    let head = signed(&mut doc, &key(1), vec![], b"root");
    let ledger = doc.ledger();
    let plan = ledger.export(&ExportCursor::new());
    let item = ledger.export_item(&plan, 0).unwrap();
    let cursor = plan.cursor();
    let Metadata::Commit(commit) = describe(TREE, &item, Blob::new(vec![1, 2, 3]).meta()).unwrap()
    else {
        panic!("expected standalone commit");
    };
    assert_eq!(commit.head(), id(head));
    assert_eq!(plan.cursor(), cursor);
    assert_eq!(ledger.export(&cursor).len(), 1);
}

#[test]
fn malformed_export_shapes_return_errors_without_panicking() {
    let (base, bytes) = standalone();
    let blob = Blob::new(bytes).meta();
    let mut cases = Vec::new();
    let mut item = base.clone();
    item.id = ItemId::Commit(CommitHash::from([99; 32]));
    cases.push(item);
    let mut item = base.clone();
    item.members.clear();
    cases.push(item);
    let mut item = base.clone();
    Arc::make_mut(&mut item.meta).members.clear();
    cases.push(item);
    let mut item = base.clone();
    item.members[0].0 = CommitHash::from([99; 32]);
    cases.push(item);
    let mut item = base.clone();
    let member = item.members[0];
    item.members.push(member);
    let meta = Arc::make_mut(&mut item.meta);
    meta.members.push(meta.members[0].clone());
    cases.push(item);
    let mut item = base.clone();
    item.checkpoints.push(item.head());
    cases.push(item);
    let mut item = base.clone();
    Arc::make_mut(&mut item.meta).members[0].parents = vec![ParentRef::Member(0)];
    cases.push(item);
    let mut item = base;
    item.id = ItemId::Fragment(FragmentRange {
        head: CommitHash::from([0; 32]),
        boundary: BTreeSet::new(),
    });
    Arc::make_mut(&mut item.meta).id = item.id.clone();
    cases.push(item);
    for item in cases {
        let error = describe(TREE, &item, blob).unwrap_err();
        assert!(matches!(error, Error::InvalidExport(_)), "{error:?}");
        assert!(error.source().is_none());
    }
}

fn unique_hash(n: usize) -> CommitHash {
    let mut bytes = [0; 32];
    bytes[1..9].copy_from_slice(&(n as u64).to_le_bytes());
    CommitHash::from(bytes)
}

#[test]
fn sedimentree_limits_are_reported_without_truncation() {
    // Synthetic export structures exercise conversion limits without building
    // tens of thousands of signed commits. describe is not an authenticator.
    let (mut item, bytes) = standalone();
    let blob = Blob::new(bytes).meta();
    Arc::make_mut(&mut item.meta).members[0].parents = (0..=MAX_PARENTS)
        .map(|n| ParentRef::External(unique_hash(n)))
        .collect();
    let error = describe(TREE, &item, blob).unwrap_err();
    assert_eq!(
        error,
        Error::Commit(InvalidLooseCommit::TooManyParents {
            actual: MAX_PARENTS + 1
        })
    );
    assert!(error.source().is_some());

    item.id = ItemId::Fragment(FragmentRange {
        head: item.head(),
        boundary: (0..=MAX_BOUNDARIES).map(unique_hash).collect(),
    });
    Arc::make_mut(&mut item.meta).id = item.id.clone();
    assert_eq!(
        describe(TREE, &item, blob),
        Err(Error::Fragment(InvalidFragment::TooManyBoundaries {
            actual: MAX_BOUNDARIES + 1
        }))
    );

    let ItemId::Fragment(range) = &mut item.id else {
        unreachable!()
    };
    range.boundary.clear();
    Arc::make_mut(&mut item.meta).id = item.id.clone();
    item.checkpoints = (0..=MAX_CHECKPOINTS).map(unique_hash).collect();
    let error = describe(TREE, &item, blob).unwrap_err();
    assert_eq!(
        error,
        Error::Fragment(InvalidFragment::TooManyCheckpoints {
            actual: MAX_CHECKPOINTS + 1
        })
    );
    assert!(error.source().is_some());
}
