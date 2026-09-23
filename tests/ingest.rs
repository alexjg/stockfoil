//! API contracts for verified sedimentree admission. These compile now but will
//! exercise todo!() stubs until verification and admission are implemented.
mod support;

use stockfoil::{
    BlobHash, CommitContents, CommitGraph, CommitHash, CommitMeta, DependencyId, FragmentRange,
    IngestError, SignatureEvidence, UnverifiedItem, VerificationError, VerifiedDependencies,
    VerifiedDependency, VerifiedItem,
};
use support::{author, key, signature};

struct NoDependencies;

impl VerifiedDependencies for NoDependencies {
    fn get(&self, _: &DependencyId) -> Option<VerifiedDependency<'_>> {
        None
    }
}

// A verifier can use one retained verified item, not a whole CommitGraph.
struct ItemDependencies<'a>(&'a VerifiedItem);

impl VerifiedDependencies for ItemDependencies<'_> {
    fn get(&self, id: &DependencyId) -> Option<VerifiedDependency<'_>> {
        self.0.dependency(id)
    }
}

fn commit(parents: Vec<CommitHash>, payload: &[u8]) -> CommitMeta {
    CommitMeta::new(CommitContents {
        parents,
        author: author(&key(1)),
        payload: BlobHash::of_contents(payload),
    })
}

fn proof(meta: &CommitMeta) -> SignatureEvidence {
    SignatureEvidence {
        author: *meta.author(),
        commit: *meta.hash(),
        signature: signature(&key(1), *meta.hash()),
    }
}

fn item(meta: &CommitMeta) -> UnverifiedItem {
    UnverifiedItem::Commit {
        meta: meta.clone(),
        signatures: vec![proof(meta)],
    }
}

#[test]
fn admit_verified_metadata_with_payload_storage_handles() {
    let meta = commit(vec![], b"payload stored elsewhere");
    let incoming = item(&meta);
    assert_eq!(incoming.dependencies().count(), 0);
    let verified = incoming.verify(&NoDependencies).unwrap();
    let mut graph = CommitGraph::new();
    graph.ingest(verified, |meta| *meta.payload_hash()).unwrap();
    let (stored, handle) = graph.publishable_commit(meta.hash()).unwrap();
    assert_eq!(stored.hash(), meta.hash());
    assert_eq!(handle, meta.payload_hash());
}

#[test]
fn isolated_verification_does_not_imply_destination_has_the_dependencies() {
    let root = commit(vec![], b"root");
    let child = commit(vec![*root.hash()], b"child");
    let root_item = item(&root).verify(&NoDependencies).unwrap();
    let incoming = item(&child);
    let id = DependencyId::Commit(*root.hash());
    assert_eq!(
        incoming.dependencies().collect::<Vec<_>>(),
        vec![id.clone()]
    );
    assert_eq!(
        incoming.clone().verify(&NoDependencies).unwrap_err(),
        VerificationError::MissingDependency(id.clone()),
    );

    let verified = incoming
        .clone()
        .verify(&ItemDependencies(&root_item))
        .unwrap();
    let mut destination = CommitGraph::new();
    let mut callbacks = 0;
    assert_eq!(
        destination.ingest(verified, |_| {
            callbacks += 1;
            ()
        }),
        Err(IngestError::MissingDependency(id)),
    );
    assert_eq!(callbacks, 0);
    assert_eq!(destination.delivered_heads().count(), 0);

    destination.ingest(root_item, |_| ()).unwrap();
    let verified = incoming.verify(&destination).unwrap();
    destination.ingest(verified, |_| ()).unwrap();
    assert!(destination.publishable_commit(child.hash()).is_some());
}

#[test]
fn a_fragment_is_verified_as_one_item_before_any_commits_are_admitted() {
    let root = (0u64..1_000)
        .map(|nonce| commit(vec![], &nonce.to_le_bytes()))
        .find(|meta| meta.hash().as_bytes()[0] != 0)
        .unwrap();
    // A real depth-one head; no synthetic hash or configurable depth metric.
    let head = (0u64..1_000_000)
        .map(|nonce| commit(vec![*root.hash()], &nonce.to_le_bytes()))
        .find(|meta| meta.hash().as_bytes()[0] == 0 && meta.hash().as_bytes()[1] != 0)
        .unwrap();
    let range = FragmentRange {
        head: *head.hash(),
        boundary: Default::default(),
    };
    let incoming = UnverifiedItem::Fragment {
        range: range.clone(),
        commits: vec![root.clone(), head.clone()],
        signatures: vec![proof(&root), proof(&head)],
    };
    let verified = incoming.verify(&NoDependencies).unwrap();
    let mut graph = CommitGraph::new();
    graph.ingest(verified, |meta| *meta.payload_hash()).unwrap();
    assert!(
        graph
            .as_sedimentree()
            .summary()
            .fragments
            .iter()
            .any(|f| f.range == range)
    );
    assert!(graph.publishable_commit(root.hash()).is_some());
    assert!(graph.publishable_commit(head.hash()).is_some());
}
