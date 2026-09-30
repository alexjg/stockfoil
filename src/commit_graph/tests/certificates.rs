use super::super::*;
use super::CommitGraph;
use ed25519_dalek::{Signer, SigningKey};

fn author(key: &SigningKey) -> Author {
    Author::try_from(key.verifying_key().as_bytes().as_slice()).unwrap()
}

fn mined(author: Author, parents: Vec<CommitHash>, depth: usize) -> CommitMeta {
    (0u64..)
        .map(|nonce| {
            CommitMeta::new(CommitContents {
                author,
                parents: parents.clone(),
                payload: BlobHash::of_contents(&nonce.to_le_bytes()),
            })
        })
        .find(|meta| sedimentree_depth(meta.hash()) == depth)
        .unwrap()
}

fn sign(graph: &mut CommitGraph<u8>, key: &SigningKey, meta: &CommitMeta) {
    graph
        .ingest_signature(
            author(key),
            *meta.hash(),
            Signature::from(key.sign(meta.hash().as_bytes()).to_bytes()),
        )
        .unwrap();
}

#[test]
fn replacement_prunes_certificates_but_preserves_authentication_and_viewpoints() {
    let key = SigningKey::from_bytes(&[1; 32]);
    let alice = author(&key);
    let mut graph = CommitGraph::new(Default::default());
    graph.set_write_permissions(&HashMap::from([(alice, WritePermission::Unbounded)]));
    let root = mined(alice, vec![], 0);
    graph.insert_local_commit(root.clone(), 1);
    sign(&mut graph, &key, &root);
    let old_view = graph.signed_view_point();
    let stale = ItemId::Commit(*root.hash());
    assert!(graph.export_item(&stale).unwrap().verify().is_ok());

    let head = mined(alice, vec![*root.hash()], 1);
    graph.insert_local_commit(head.clone(), 2);
    sign(&mut graph, &key, &head);
    let cover = graph.minimal_cover();
    assert!(cover.loose_commits.is_empty());
    assert_eq!(cover.fragments.len(), 1);
    assert!(
        matches!(graph.export_item(&stale), Err(ExportError::NotInMinimalCover(id)) if id == stale)
    );
    assert!(
        graph.nodes[graph.by_hash[root.hash()]]
            .signatures
            .is_empty()
    );
    let fragment = graph.nodes[graph.by_hash[head.hash()]]
        .canonical_fragment()
        .unwrap();
    assert_eq!(
        graph.fragments[fragment].signature_boundary[&alice],
        vec![graph.by_hash[head.hash()]]
    );
    assert_eq!(graph.signed_view_point().heads(), &[*head.hash()]);
    assert!(graph.has_commit(root.hash()));
    let mut peer = CommitGraph::new(Default::default());
    peer.merge_fixture(&graph);
    assert_eq!(peer.minimal_cover(), cover);
    let mut relay = CommitGraph::new(Default::default());
    relay.merge_fixture(&peer);
    assert_eq!(relay.minimal_cover(), cover);

    // Pruning export certificates must not change captured membership,
    // including on replicas that only received the replacement fragment.
    for graph in [&graph, &peer, &relay] {
        let membership = graph.diff(&old_view, &old_view).unwrap();
        assert_eq!(membership.visibility(root.hash()), Ok(crate::Vis::Both));
        assert_eq!(membership.visibility(head.hash()), Ok(crate::Vis::Neither));
    }
}

#[test]
fn pending_local_checkpoint_does_not_enter_the_index_before_admission() {
    let key = SigningKey::from_bytes(&[1; 32]);
    let alice = author(&key);
    let mut graph = CommitGraph::new(Default::default());
    let root = mined(alice, vec![], 0);
    graph.insert_local_commit(root.clone(), 1);
    sign(&mut graph, &key, &root);
    let old_cover = graph.minimal_cover();
    let head = mined(alice, vec![*root.hash()], 1);
    let tip = mined(alice, vec![*head.hash()], 0);
    // Local work is delivered parents-first, but signatures can arrive in
    // the opposite order without publishing unsigned dependencies.
    graph.insert_local_commit(head.clone(), 2);
    graph.insert_local_commit(tip.clone(), 3);
    sign(&mut graph, &key, &tip);
    assert!(!graph.has_commit(head.hash()));
    assert!(!graph.has_commit(tip.hash()));
    assert!(
        graph.nodes[graph.by_hash[head.hash()]]
            .headed_fragments
            .is_empty()
    );
    // Neither pending item has entered the authenticated index.
    assert_eq!(graph.minimal_cover(), old_cover);
    assert!(graph.export_item(&ItemId::Commit(*root.hash())).is_ok());
    assert!(graph.export_item(&ItemId::Commit(*tip.hash())).is_err());
    assert_eq!(graph.nodes[graph.by_hash[tip.hash()]].signatures.len(), 1);

    sign(&mut graph, &key, &head);
    assert_ne!(graph.minimal_cover(), old_cover);
    assert!(graph.has_commit(tip.hash()));
    assert!(graph.export_item(&ItemId::Commit(*root.hash())).is_err());
    let mut peer = CommitGraph::new(Default::default());
    peer.merge_fixture(&graph);
    assert!(peer.has_commit(tip.hash()));
    assert_eq!(peer.minimal_cover(), graph.minimal_cover());
}

#[test]
fn pending_local_siblings_share_evidence_before_constituents_are_discarded() {
    let alice_key = SigningKey::from_bytes(&[1; 32]);
    let bob_key = SigningKey::from_bytes(&[2; 32]);
    let root = mined(author(&alice_key), vec![], 0);
    let left = mined(author(&bob_key), vec![*root.hash()], 1);
    let right = mined(author(&bob_key), vec![*root.hash(), *root.hash()], 1);
    let mut graph = CommitGraph::new(Default::default());
    for meta in [&root, &left, &right] {
        graph.insert_local_commit(meta.clone(), 1);
    }
    sign(&mut graph, &bob_key, &left);
    sign(&mut graph, &bob_key, &right);
    assert!(graph.minimal_cover().fragments.is_empty());
    sign(&mut graph, &alice_key, &root);
    assert_eq!(graph.minimal_cover().fragments.len(), 2);
    assert!(graph.pending_local.is_empty());
    assert!(
        graph.nodes[graph.by_hash[root.hash()]]
            .signatures
            .contains_key(&author(&alice_key))
    );
    let mut peer = CommitGraph::new(Default::default());
    peer.merge_fixture(&graph);
    assert_eq!(peer.minimal_cover(), graph.minimal_cover());
}

#[test]
fn shared_signature_survives_on_its_target_for_each_current_entry() {
    let key = SigningKey::from_bytes(&[1; 32]);
    let bob_key = SigningKey::from_bytes(&[2; 32]);
    let alice = author(&key);
    let mut graph = CommitGraph::new(Default::default());
    let root = mined(author(&bob_key), vec![], 0);
    graph.insert_local_commit(root.clone(), 1);
    sign(&mut graph, &bob_key, &root);
    let left = mined(alice, vec![*root.hash()], 1);
    // Change the ancestry so mining cannot return the same head.
    let middle = mined(alice, vec![*root.hash()], 0);
    let right = mined(alice, vec![*middle.hash()], 1);
    graph.insert_local_commit(left.clone(), 2);
    graph.insert_local_commit(middle.clone(), 3);
    graph.insert_local_commit(right.clone(), 4);
    sign(&mut graph, &key, &left);
    // The second received fragment carries its own certificate, including
    // evidence shared with the first. No historical signature insertion.
    let incoming = UnverifiedItem::Fragment {
        checkpoints: BTreeSet::new(),
        range: FragmentRange {
            head: *right.hash(),
            boundary: BTreeSet::new(),
        },
        commits: vec![root.clone(), middle.clone(), right.clone()],
        signatures: vec![
            SignatureEvidence {
                author: author(&bob_key),
                commit: *root.hash(),
                signature: Signature::from(bob_key.sign(root.hash().as_bytes()).to_bytes()),
            },
            SignatureEvidence {
                author: alice,
                commit: *right.hash(),
                signature: Signature::from(key.sign(right.hash().as_bytes()).to_bytes()),
            },
        ],
    }
    .verify()
    .unwrap();
    graph.ingest_verified(incoming, |_| 0).unwrap();
    assert_eq!(graph.minimal_cover().fragments.len(), 2);
    for fragment in graph.minimal_cover().fragments {
        let idx = graph.nodes[graph.by_hash[&fragment.range.head]]
            .canonical_fragment()
            .unwrap();
        assert!(
            graph.fragments[idx].signature_boundary[&author(&bob_key)]
                .contains(&graph.by_hash[root.hash()])
        );
        assert!(
            graph.nodes[graph.by_hash[root.hash()]]
                .signatures
                .contains_key(&author(&bob_key))
        );
        assert!(
            graph
                .export_item(&ItemId::Fragment(fragment.range))
                .unwrap()
                .verify()
                .is_ok()
        );
    }
    let mut peer = CommitGraph::new(Default::default());
    peer.merge_fixture(&graph);
    assert!(peer.has_commit(root.hash()));
}
