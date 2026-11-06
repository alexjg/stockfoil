//! Items published to and read back from an in-memory Subduction node.
#[path = "../../stockfoil/tests/support/mod.rs"]
mod support;

use ed25519_dalek::SigningKey;
use sedimentree_core::blob::Blob;
use sedimentree_core::fragment::Fragment;
use sedimentree_core::id::SedimentreeId;
use sedimentree_core::loose_commit::LooseCommit;
use std::collections::{BTreeSet, HashMap};
use stockfoil::*;
use stockfoil_subduction::{Metadata, commit_id, export_item};
use subduction_core::connection::test_utils::new_test_subduction;
use support::{ChunkCodec, key, payload_at_depth};

type SubDocument = Document<ChunkCodec>;
const TREE: SedimentreeId = SedimentreeId::new([7; 32]);
fn document() -> SubDocument {
    support::document()
}
fn signed(
    g: &mut SubDocument,
    key: &SigningKey,
    parents: Vec<CommitHash>,
    payload: &[u8],
) -> CommitHash {
    support::signed(g, key, parents, payload)
}

/// A stand-in for encryption: the blob Subduction stores is not the item.
fn mask(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().map(|b| b ^ 0x5a).collect()
}

type Batch = (
    Vec<(LooseCommit, Blob)>,
    Vec<(Fragment, Blob)>,
    ExportCursor,
);

/// Everything `cursor` lacks, as Subduction batch inputs.
fn export(g: &SubDocument, cursor: &ExportCursor, seal: impl Fn(Vec<u8>) -> Blob) -> Batch {
    let mut plan = g.export(cursor);
    let (mut commits, mut fragments) = (vec![], vec![]);
    for i in 0..plan.len() {
        match export_item(g, plan.plan(), i, TREE, &seal).unwrap() {
            (Metadata::Commit(c), blob) => commits.push((c, blob)),
            (Metadata::Fragment(f), blob) => fragments.push((f, blob)),
        }
        plan.acknowledge(i).unwrap();
    }
    (commits, fragments, plan.cursor())
}

fn offered(commits: Option<Vec<LooseCommit>>, fragments: Option<Vec<Fragment>>) -> Vec<Metadata> {
    let commits = commits
        .unwrap_or_default()
        .into_iter()
        .map(Metadata::Commit);
    let fragments = fragments
        .unwrap_or_default()
        .into_iter()
        .map(Metadata::Fragment);
    commits.chain(fragments).collect()
}

fn hashes(receipt: &Receipt) -> Vec<CommitHash> {
    receipt.members.iter().map(|m| m.hash).collect()
}

/// Receive items in any order and check each admitted item's metadata.
fn receive_all(
    g: &mut SubDocument,
    items: Vec<(Metadata, Vec<u8>)>,
) -> Vec<(Metadata, stockfoil::Error)> {
    let mut by_head: HashMap<CommitHash, Metadata> = HashMap::new();
    let mut failed = Vec::new();
    for (metadata, bytes) in items {
        let head = CommitHash::from(*metadata.head().as_bytes());
        by_head.insert(head, metadata.clone());
        match g.receive(&bytes) {
            Ok(Received::Admitted {
                items: receipts, ..
            }) => {
                for r in receipts {
                    if let Some(m) = by_head.remove(&r.id.head()) {
                        m.check(g.history(), &r.id, &hashes(&r)).unwrap();
                    }
                }
            }
            Ok(Received::Waiting(_) | Received::Evicted { .. }) => {}
            Err(e) => {
                by_head.remove(&head);
                failed.push((metadata, e));
            }
        }
    }
    for m in by_head.into_values() {
        failed.push((
            m,
            stockfoil::Error::MissingDependency(CommitHash::from([0; 32])),
        ));
    }
    failed
}

fn heads(items: &[Metadata]) -> BTreeSet<CommitHash> {
    items
        .iter()
        .map(|m| CommitHash::from(*m.head().as_bytes()))
        .collect()
}

#[tokio::test]
async fn items_round_trip_and_minimize_in_subduction() {
    let (node, _listener, _manager) = new_test_subduction();
    let (alice, bob) = (key(1), key(2));
    let mut a = document();

    let mut tip = signed(&mut a, &alice, vec![], b"root");
    for i in 0..3 {
        tip = signed(&mut a, &alice, vec![tip], format!("a{i}").as_bytes());
    }
    let h1 = signed(
        &mut a,
        &alice,
        vec![tip],
        &payload_at_depth(&alice, &[tip], 1, "h1"),
    );
    let (commits, fragments, cursor) = export(&a, &ExportCursor::new(), Blob::new);
    assert_eq!((commits.len(), fragments.len()), (0, 1));
    node.store_built_batch(TREE, commits, fragments)
        .await
        .unwrap();

    let b1 = signed(&mut a, &bob, vec![h1], b"bob");
    let a1 = signed(&mut a, &alice, vec![h1], b"alice");
    let merge = signed(&mut a, &alice, vec![b1, a1], b"merge");
    let (commits, fragments, cursor) = export(&a, &cursor, Blob::new);
    assert_eq!((commits.len(), fragments.len()), (3, 0));
    node.store_built_batch(TREE, commits, fragments)
        .await
        .unwrap();

    let h2 = signed(
        &mut a,
        &alice,
        vec![merge],
        &payload_at_depth(&alice, &[merge], 2, "h2"),
    );
    let tail = signed(&mut a, &bob, vec![h2], b"tail");
    let (commits, fragments, _) = export(&a, &cursor, Blob::new);
    let checkpoints: Vec<_> = fragments
        .iter()
        .map(|(f, _)| f.checkpoints().len())
        .collect();
    assert_eq!(
        checkpoints,
        vec![1],
        "the depth-1 head is the fragment's one checkpoint"
    );
    node.store_built_batch(TREE, commits, fragments)
        .await
        .unwrap();

    let offered = offered(node.get_commits(TREE).await, node.get_fragments(TREE).await);
    assert_eq!(heads(&offered), BTreeSet::from([h2, tail, b1, a1, merge]));

    let mut b = document();
    let mut fetched = Vec::new();
    for metadata in offered {
        let digest = metadata.blob_meta().digest();
        let blob = node.get_blob(TREE, digest).await.unwrap().unwrap();
        fetched.push((metadata, blob.into_contents()));
    }
    assert!(receive_all(&mut b, fetched).is_empty());
    let delivered: Vec<_> = a.history().traverse().collect();
    assert_eq!(delivered.len(), 10);
    for hash in delivered {
        assert!(b.history().is_admitted(&hash));
        assert_eq!(b.payload(&hash).unwrap(), a.payload(&hash).unwrap());
    }
}

#[tokio::test]
async fn sealed_blobs_are_keyed_by_what_subduction_stores() {
    let (node, _listener, _manager) = new_test_subduction();
    let alice = key(1);
    let mut a = document();
    let root = signed(&mut a, &alice, vec![], b"root");
    let head = signed(
        &mut a,
        &alice,
        vec![root],
        &payload_at_depth(&alice, &[root], 1, "head"),
    );
    let child = signed(&mut a, &alice, vec![head], b"child");
    let (commits, fragments, _) = export(&a, &ExportCursor::new(), |bytes| Blob::new(mask(&bytes)));
    node.store_built_batch(TREE, commits, fragments)
        .await
        .unwrap();

    let mut b = document();
    let offered = offered(node.get_commits(TREE).await, node.get_fragments(TREE).await);
    let mut unsealed = Vec::new();
    for metadata in offered {
        let digest = metadata.blob_meta().digest();
        let sealed = node.get_blob(TREE, digest).await.unwrap().unwrap();
        let item = mask(sealed.as_slice());
        assert_ne!(Blob::new(item.clone()).meta().digest(), digest);
        unsealed.push((metadata, item));
    }
    assert!(receive_all(&mut b, unsealed).is_empty());
    for hash in [root, head, child] {
        assert_eq!(b.payload(&hash).unwrap(), a.payload(&hash).unwrap());
    }
}

#[test]
fn metadata_that_misdescribes_an_item_is_rejected() {
    let alice = key(1);
    let mut a = document();
    let root = signed(&mut a, &alice, vec![], b"root");
    let d1 = signed(
        &mut a,
        &alice,
        vec![root],
        &payload_at_depth(&alice, &[root], 1, "d1"),
    );
    let mid = signed(&mut a, &alice, vec![d1], b"mid");
    let d2 = signed(
        &mut a,
        &alice,
        vec![mid],
        &payload_at_depth(&alice, &[mid], 2, "d2"),
    );
    let tail = signed(&mut a, &alice, vec![d2], b"tail");
    let plan = a.export(&ExportCursor::new());
    assert_eq!(plan.len(), 2);
    let items: Vec<_> = (0..2)
        .map(|i| export_item(&a, plan.plan(), i, TREE, Blob::new).unwrap().0)
        .collect();
    let members = |id: &ItemId| -> Vec<CommitHash> {
        match id {
            ItemId::Commit(h) => vec![*h],
            ItemId::Fragment(_) => vec![root, d1, mid, d2],
        }
    };
    let fragment_id = ItemId::Fragment(FragmentRange {
        head: d2,
        boundary: BTreeSet::new(),
    });
    for m in &items {
        let id = match m {
            Metadata::Commit(_) => ItemId::Commit(tail),
            Metadata::Fragment(_) => fragment_id.clone(),
        };
        m.check(a.history(), &id, &members(&id)).unwrap();
    }

    let Metadata::Fragment(fragment) = items
        .iter()
        .find(|m| matches!(m, Metadata::Fragment(_)))
        .unwrap()
    else {
        unreachable!()
    };
    let blob = fragment.summary().blob_meta();
    let lies = [
        Fragment::new(
            TREE,
            commit_id(&d2),
            BTreeSet::new(),
            &[commit_id(&d1), commit_id(&tail)],
            blob,
        ),
        Fragment::new(TREE, commit_id(&d2), BTreeSet::new(), &[], blob),
        Fragment::new(
            TREE,
            commit_id(&d2),
            BTreeSet::from([commit_id(&mid)]),
            &[commit_id(&d1)],
            blob,
        ),
    ];
    for lie in lies {
        let lie = Metadata::Fragment(lie.unwrap());
        assert_eq!(
            lie.check(a.history(), &fragment_id, &members(&fragment_id)),
            Err(stockfoil_subduction::Error::Mismatch(d2))
        );
    }
    let Metadata::Commit(commit) = items
        .iter()
        .find(|m| matches!(m, Metadata::Commit(_)))
        .unwrap()
    else {
        unreachable!()
    };
    let lie = LooseCommit::new(
        TREE,
        commit_id(&tail),
        BTreeSet::from([commit_id(&mid)]),
        *commit.blob_meta(),
    );
    assert_eq!(
        Metadata::Commit(lie.unwrap()).check(a.history(), &ItemId::Commit(tail), &[tail]),
        Err(stockfoil_subduction::Error::Mismatch(tail))
    );
}

#[tokio::test]
async fn corrupt_blobs_fail_without_disturbing_the_rest() {
    let (node, _listener, _manager) = new_test_subduction();
    let alice = key(1);
    let mut a = document();
    let root = signed(&mut a, &alice, vec![], b"root");
    let head = signed(
        &mut a,
        &alice,
        vec![root],
        &payload_at_depth(&alice, &[root], 1, "head"),
    );
    let (commits, fragments, cursor) = export(&a, &ExportCursor::new(), Blob::new);
    node.store_built_batch(TREE, commits, fragments)
        .await
        .unwrap();
    let side = signed(&mut a, &alice, vec![root], b"side");
    let (commits, fragments, _) = export(&a, &cursor, Blob::new);
    node.store_built_batch(TREE, commits, fragments)
        .await
        .unwrap();

    let mut b = document();
    let offered = offered(node.get_commits(TREE).await, node.get_fragments(TREE).await);
    assert_eq!(heads(&offered), BTreeSet::from([head, side]));
    let mut corrupted = Vec::new();
    for metadata in &offered {
        let digest = metadata.blob_meta().digest();
        let mut bytes = node
            .get_blob(TREE, digest)
            .await
            .unwrap()
            .unwrap()
            .into_contents();
        if metadata.head() == commit_id(&head) {
            let last = bytes.len() - 5;
            bytes[last] ^= 1;
        }
        corrupted.push((metadata.clone(), bytes));
    }
    let failed = receive_all(&mut b, corrupted);
    assert_eq!(failed.len(), 2, "{failed:?}");
    assert!(b.history().is_empty());
    assert_eq!(b.ledger().parked(), 1);

    // Receiving the good fragment unblocks `side` from the queue.
    let mut good = Vec::new();
    for metadata in offered {
        let digest = metadata.blob_meta().digest();
        let blob = node.get_blob(TREE, digest).await.unwrap().unwrap();
        good.push((metadata, blob.into_contents()));
    }
    assert!(receive_all(&mut b, good).is_empty());
    assert_eq!(b.ledger().parked(), 0);
    assert_eq!(b.payload(&head).unwrap(), a.payload(&head).unwrap());
    assert_eq!(b.payload(&side).unwrap().as_ref(), b"side");
}
