//! Items published to and read back from an in-memory Subduction node.
#[path = "../../tests/support/mod.rs"]
mod support;

use ed25519_dalek::SigningKey;
use sedimentree_core::blob::Blob;
use sedimentree_core::fragment::Fragment;
use sedimentree_core::id::SedimentreeId;
use sedimentree_core::loose_commit::LooseCommit;
use std::collections::BTreeSet;
use stockfoil::*;
use stockfoil_subduction::{Metadata, SubductionStorage, commit_id, export_item};
use subduction_core::connection::test_utils::new_test_subduction;
use support::{ChunkCodec, author, key, payload_at_depth, sign};

type SubGraph = Graph<ChunkCodec, SubductionStorage>;

const TREE: SedimentreeId = SedimentreeId::new([7; 32]);

fn graph() -> SubGraph {
    Graph::new(ChunkCodec::default(), SubductionStorage::new())
}

fn signed(
    g: &mut SubGraph,
    key: &SigningKey,
    parents: Vec<CommitHash>,
    payload: &[u8],
) -> CommitHash {
    let prepared = g.commit(author(key), parents, payload).unwrap();
    g.sign(author(key), prepared.hash, sign(key, &prepared.hash))
        .unwrap();
    prepared.hash
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
fn export(g: &mut SubGraph, cursor: &ExportCursor, seal: impl Fn(Vec<u8>) -> Blob) -> Batch {
    let mut plan = g.export(cursor);
    let (mut commits, mut fragments) = (vec![], vec![]);
    for i in 0..plan.len() {
        match export_item(g, &plan, i, TREE, &seal).unwrap() {
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

/// Admit held items in any order, retrying while that makes progress, and
/// check each admitted item's metadata. Returns what never admitted, and why.
fn admit_all(g: &mut SubGraph, items: Vec<Metadata>) -> Vec<(Metadata, stockfoil::Error)> {
    let mut pending = items;
    loop {
        let before = pending.len();
        let mut left = vec![];
        for metadata in pending {
            match g.admit(&metadata.blob_meta().digest()) {
                Ok(receipt) => metadata
                    .check(g.history(), &receipt.id, &hashes(&receipt))
                    .unwrap(),
                Err(e) => left.push((metadata, e)),
            }
        }
        if left.len() == before || left.is_empty() {
            return left;
        }
        pending = left.into_iter().map(|(m, _)| m).collect();
    }
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
    let mut a = graph();

    // Round 1: a depth-1 fragment over a short chain.
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
    let (commits, fragments, cursor) = export(&mut a, &ExportCursor::new(), Blob::new);
    assert_eq!((commits.len(), fragments.len()), (0, 1));
    node.store_built_batch(TREE, commits, fragments)
        .await
        .unwrap();

    // Round 2: loose commits, including a merge of two authors.
    let b1 = signed(&mut a, &bob, vec![h1], b"bob");
    let a1 = signed(&mut a, &alice, vec![h1], b"alice");
    let merge = signed(&mut a, &alice, vec![b1, a1], b"merge");
    let (commits, fragments, cursor) = export(&mut a, &cursor, Blob::new);
    assert_eq!((commits.len(), fragments.len()), (3, 0));
    node.store_built_batch(TREE, commits, fragments)
        .await
        .unwrap();

    // Round 3: a depth-2 fragment covering everything so far, then a tail.
    let h2 = signed(
        &mut a,
        &alice,
        vec![merge],
        &payload_at_depth(&alice, &[merge], 2, "h2"),
    );
    let tail = signed(&mut a, &bob, vec![h2], b"tail");
    let (commits, fragments, _) = export(&mut a, &cursor, Blob::new);
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

    // Subduction's minimization reads the checkpoints: the depth-2 fragment
    // supports the depth-1 fragment, which is no longer offered. It keeps the
    // loose commits the depth-2 fragment covers, because it only prunes loose
    // commits below a fragment head that is itself stored as a loose commit.
    // Receivers admit them again as duplicates.
    let offered = offered(node.get_commits(TREE).await, node.get_fragments(TREE).await);
    assert_eq!(heads(&offered), BTreeSet::from([h2, tail, b1, a1, merge]));

    // A receiver fetches the offered blobs, admits them, and checks metadata.
    let mut b = graph();
    for metadata in &offered {
        let digest = metadata.blob_meta().digest();
        let blob = node.get_blob(TREE, digest).await.unwrap().unwrap();
        assert_eq!(b.storage_mut().insert_blob(blob), digest);
    }
    assert!(admit_all(&mut b, offered).is_empty());
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
    let mut a = graph();
    let root = signed(&mut a, &alice, vec![], b"root");
    let head = signed(
        &mut a,
        &alice,
        vec![root],
        &payload_at_depth(&alice, &[root], 1, "head"),
    );
    let child = signed(&mut a, &alice, vec![head], b"child");
    let (commits, fragments, _) = export(&mut a, &ExportCursor::new(), |bytes| {
        Blob::new(mask(&bytes))
    });
    node.store_built_batch(TREE, commits, fragments)
        .await
        .unwrap();

    let mut b = graph();
    let offered = offered(node.get_commits(TREE).await, node.get_fragments(TREE).await);
    for metadata in &offered {
        let digest = metadata.blob_meta().digest();
        let sealed = node.get_blob(TREE, digest).await.unwrap().unwrap();
        let item = Blob::new(mask(sealed.as_slice()));
        assert_ne!(item.meta().digest(), digest);
        b.storage_mut().insert(digest, item);
    }
    assert!(admit_all(&mut b, offered).is_empty());
    for hash in [root, head, child] {
        assert_eq!(b.payload(&hash).unwrap(), a.payload(&hash).unwrap());
    }
}

#[test]
fn metadata_that_misdescribes_an_item_is_rejected() {
    let alice = key(1);
    let mut a = graph();
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
        .map(|i| export_item(&mut a, &plan, i, TREE, Blob::new).unwrap().0)
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
        // A checkpoint the item lacks would let Subduction prune `tail`.
        Fragment::new(
            TREE,
            commit_id(&d2),
            BTreeSet::new(),
            &[commit_id(&d1), commit_id(&tail)],
            blob,
        ),
        // A missing checkpoint.
        Fragment::new(TREE, commit_id(&d2), BTreeSet::new(), &[], blob),
        // A boundary the item does not stop at.
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
    let mut a = graph();
    let root = signed(&mut a, &alice, vec![], b"root");
    let head = signed(
        &mut a,
        &alice,
        vec![root],
        &payload_at_depth(&alice, &[root], 1, "head"),
    );
    let (commits, fragments, cursor) = export(&mut a, &ExportCursor::new(), Blob::new);
    node.store_built_batch(TREE, commits, fragments)
        .await
        .unwrap();
    let side = signed(&mut a, &alice, vec![root], b"side");
    let (commits, fragments, _) = export(&mut a, &cursor, Blob::new);
    node.store_built_batch(TREE, commits, fragments)
        .await
        .unwrap();

    let mut b = graph();
    let offered = offered(node.get_commits(TREE).await, node.get_fragments(TREE).await);
    assert_eq!(heads(&offered), BTreeSet::from([head, side]));
    for metadata in &offered {
        let digest = metadata.blob_meta().digest();
        let mut bytes = node
            .get_blob(TREE, digest)
            .await
            .unwrap()
            .unwrap()
            .into_contents();
        if metadata.head() == commit_id(&head) {
            // Decrypted to something that is not the item.
            let last = bytes.len() - 5;
            bytes[last] ^= 1;
        }
        b.storage_mut().insert(digest, Blob::new(bytes));
    }
    let failed = admit_all(&mut b, offered.clone());
    // The fragment fails verification; `side` then waits on its root forever.
    assert_eq!(failed.len(), 2, "{failed:?}");
    assert!(b.history().is_empty());

    // Good bytes later admit both.
    for metadata in &offered {
        let digest = metadata.blob_meta().digest();
        let blob = node.get_blob(TREE, digest).await.unwrap().unwrap();
        b.storage_mut().insert(digest, blob);
    }
    assert!(admit_all(&mut b, offered.clone()).is_empty());

    // Bytes that change after admission fail reads instead of returning them.
    let fragment = offered
        .iter()
        .find(|m| m.head() == commit_id(&head))
        .unwrap();
    let digest = fragment.blob_meta().digest();
    let mut bytes = b.storage().get(&digest).unwrap().as_slice().to_vec();
    let last = bytes.len() - 5;
    bytes[last] ^= 1;
    b.storage_mut().insert(digest, Blob::new(bytes));
    assert!(matches!(b.payload(&head), Err(stockfoil::Error::CorruptPayload(h)) if h == head));
    assert_eq!(b.payload(&side).unwrap(), b"side");
}
