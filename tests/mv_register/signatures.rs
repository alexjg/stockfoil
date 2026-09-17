//! Red contract test: local edits don't wait for signing, publication does.

use std::{
    collections::{BTreeSet, HashMap},
    ops::ControlFlow,
};

use ed25519_dalek::{Signer, SigningKey};
use stockfoil::{Author, CommitHash, Signature, Vis, VisDiff, WritePermission};

use super::MvRegisterMap;

fn sign(key: &SigningKey, hash: &CommitHash) -> Signature {
    Signature::from(key.sign(hash.as_bytes()).to_bytes())
}

fn values(value: &str) -> BTreeSet<String> {
    BTreeSet::from([value.to_owned()])
}

fn publishable(doc: &MvRegisterMap) -> Vec<CommitHash> {
    let mut hashes = Vec::new();
    doc.graph.traverse_publishable(|meta, _| {
        hashes.push(*meta.hash());
        ControlFlow::Continue(())
    });
    hashes
}

#[test]
fn local_edits_appear_before_signing_and_a_frontier_signature_publishes_the_batch() {
    // Fixed seed for reproducibility. Sign actual commit hashes, then hold back
    // delivery to simulate the application's asynchronous signer.
    let signing_key = SigningKey::from_bytes(&[42; 32]);
    let author = Author::try_from(signing_key.verifying_key().as_bytes().as_slice()).unwrap();
    let mut local = MvRegisterMap::with_author(author);
    let mut peer = MvRegisterMap::new();
    let permissions = HashMap::from([(author, WritePermission::Unbounded)]);
    local.set_write_permissions(&permissions);
    peer.set_write_permissions(&permissions);

    let base = local.prepare_set("title", "before");
    let base_signature = sign(&signing_key, &base);
    assert_eq!(local.get("title"), values("before"));
    assert!(publishable(&local).is_empty());
    assert!(local.graph.resolve(&local.graph.pov()).is_err());
    local
        .ingest_signature(author, base, base_signature)
        .unwrap();
    let initial_working = local.graph.pov();
    let initial_signed = local.graph.signed_pov();
    assert!(local.graph.resolve(&initial_working).is_ok());
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("before"));

    // First edit: visible locally, but no signature has arrived.
    let first = local.prepare_set("title", "pending");
    let first_working = local.graph.pov();
    let first_signed = local.graph.signed_pov();
    assert_eq!(local.get("title"), values("pending"));
    assert_eq!(
        local.get_at(&first_signed, "title").unwrap(),
        values("before")
    );
    assert!(first_working.delivered_heads() == first_signed.delivered_heads());
    assert!(local.graph.resolve(&first_working).is_err());
    assert!(local.graph.resolve(&first_signed).is_ok());
    assert!(publishable(&local) == vec![base]);
    peer.merge(&local);
    assert!(peer.graph.delivered_heads().collect::<Vec<_>>() == vec![base]);
    assert_eq!(peer.get("title"), values("before"));

    let pending = local.graph.diff(&first_signed, &first_working).unwrap();
    assert_eq!(pending.visibility(&base).unwrap(), Vis::Both);
    assert_eq!(pending.visibility(&first).unwrap(), Vis::After);
    let changes = pending.changed_commits().collect::<Vec<_>>();
    assert_eq!(changes.len(), 1);
    assert!(changes[0].commit_id == first);
    assert_eq!(changes[0].visibility_delta, VisDiff::After);

    // A valid signature for the wrong hash must not satisfy the request.
    assert!(
        local
            .ingest_signature(author, first, base_signature)
            .is_err()
    );
    assert_eq!(local.get("title"), values("pending"));
    assert!(local.graph.resolve(&first_working).is_err());
    assert!(publishable(&local) == vec![base]);
    let still_signed = local.graph.signed_pov();
    assert_eq!(
        local
            .graph
            .diff(&first_signed, &still_signed)
            .unwrap()
            .changed_commits()
            .count(),
        0
    );

    // Another keystroke overwrites the first pending value. Neither commit
    // can leak through export or merge, including through the descendant.
    let tip = local.prepare_set("title", "after");
    let tip_signature = sign(&signing_key, &tip);
    let batch_working = local.graph.pov();
    let batch_signed = local.graph.signed_pov();
    assert_eq!(local.get("title"), values("after"));
    assert_eq!(
        local.get_at(&batch_signed, "title").unwrap(),
        values("before")
    );
    assert!(local.graph.resolve(&batch_working).is_err());
    assert!(publishable(&local) == vec![base]);
    peer.merge(&local);
    assert!(peer.graph.delivered_heads().collect::<Vec<_>>() == vec![base]);
    assert_eq!(peer.get("title"), values("before"));

    // The signing-status diff includes the overwritten edit too, even though
    // only the final assignment is currently a visible register value.
    let pending = local.graph.diff(&batch_signed, &batch_working).unwrap();
    let changes = pending.changed_commits().collect::<Vec<_>>();
    assert_eq!(changes.len(), 2);
    assert!(changes.iter().all(|c| c.visibility_delta == VisDiff::After));
    assert!(
        changes.iter().map(|c| c.commit_id).collect::<BTreeSet<_>>()
            == BTreeSet::from([first, tip])
    );

    // Deliver ONLY the tip's signature: it authenticates both pending edits.
    // The first pending commit is never signed directly.
    local.ingest_signature(author, tip, tip_signature).unwrap();
    let published_working = local.graph.pov();
    let published_signed = local.graph.signed_pov();
    assert_eq!(local.get("title"), values("after"));
    assert_eq!(
        local.get_at(&published_signed, "title").unwrap(),
        values("after")
    );
    assert!(batch_working.delivered_heads() == published_working.delivered_heads());
    assert_eq!(
        local
            .graph
            .diff(&batch_working, &published_working)
            .unwrap()
            .changed_commits()
            .count(),
        0
    );
    assert_eq!(
        local
            .graph
            .diff(&published_signed, &published_working)
            .unwrap()
            .changed_commits()
            .count(),
        0
    );
    let exported = publishable(&local);
    assert_eq!(exported.len(), 3);
    assert!(exported.into_iter().collect::<BTreeSet<_>>() == BTreeSet::from([base, first, tip]));

    // Old pending POVs can now resolve, without changing their meaning.
    assert!(local.graph.resolve(&first_working).is_ok());
    assert!(local.graph.resolve(&batch_working).is_ok());
    for pov in [
        &initial_working,
        &initial_signed,
        &first_signed,
        &batch_signed,
    ] {
        assert_eq!(local.get_at(pov, "title").unwrap(), values("before"));
    }
    assert_eq!(
        local.get_at(&first_working, "title").unwrap(),
        values("pending")
    );
    assert_eq!(
        local.get_at(&batch_working, "title").unwrap(),
        values("after")
    );
    let old_signed = local.graph.resolve(&batch_signed).unwrap();
    assert!(!old_signed.visible(&first).unwrap());
    assert!(!old_signed.visible(&tip).unwrap());

    // Publication transfers the evidence, not an unsigned local allowance.
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("after"));
    assert!(peer.graph.resolve(&batch_working).is_ok());
    assert_eq!(
        peer.get_at(&first_working, "title").unwrap(),
        values("pending")
    );
    assert_eq!(
        peer.get_at(&batch_signed, "title").unwrap(),
        values("before")
    );

    // Retrying the signing response and merge is harmless.
    local.ingest_signature(author, tip, tip_signature).unwrap();
    peer.merge(&local);
    assert_eq!(peer.get("title"), values("after"));
    assert_eq!(publishable(&local).len(), 3);
    assert_eq!(
        local
            .graph
            .diff(&published_signed, &local.graph.signed_pov())
            .unwrap()
            .changed_commits()
            .count(),
        0
    );
}
