use super::*;
use crate::Assignment;
use stockfoil::ItemId;
use subduction_core::storage::traits::Storage;
use future_form::Sendable;

fn inventory(doc: &MvRegisterMap) -> Vec<stockfoil::EncodedItem> {
    doc.graph
        .export_changes(&ExportCursor::default())
        .unwrap()
        .0
        .added
}

fn set_loose(doc: &mut MvRegisterMap, label: &str) -> CommitHash {
    for nonce in 0..100_000 {
        let value = format!("{label}-{nonce}");
        let assignment = Assignment {
            key: "title".into(),
            value: value.clone(),
        };
        let meta = doc
            .graph
            .describe_commit(doc.author(), &assignment)
            .unwrap();
        if meta.hash().as_bytes()[0] != 0 {
            return doc.set("title", &value).unwrap();
        }
    }
    panic!("could not construct loose fixture")
}

// Force fragment coverage rather than relying on a randomly occurring checkpoint.
fn add_checkpoint(doc: &mut MvRegisterMap) {
    add_checkpoint_at_depth(doc, 1);
}

fn add_checkpoint_at_depth(doc: &mut MvRegisterMap, depth: usize) {
    for nonce in 0..10_000_000u64 {
        let value = nonce.to_string();
        let assignment = Assignment {
            key: "checkpoint".into(),
            value: value.clone(),
        };
        let meta = doc
            .graph
            .describe_commit(doc.author(), &assignment)
            .unwrap();
        if meta.hash().as_bytes()[..depth]
            .iter()
            .all(|byte| *byte == 0)
            && meta.hash().as_bytes()[depth] != 0
        {
            doc.set("checkpoint", &value).unwrap();
            return;
        }
    }
    panic!("could not construct checkpoint fixture")
}

#[test]
fn fragment_payloads_verify_relay_and_replay() {
    let mut source = MvRegisterMap::new();
    set_loose(&mut source, "before");
    add_checkpoint(&mut source);
    let item = inventory(&source).remove(0);
    assert!(matches!(item.metadata, TransportMetadata::Fragment { .. }));
    let id = item.metadata.id();
    let bytes = item.bytes;
    let mut peer = MvRegisterMap::new();
    peer.set_write_permissions(source.view_point().write_permissions());
    assert_eq!(admit(&mut peer, vec![bytes.clone()]).unwrap(), 0);
    let cursor = peer.materialized.cursor.clone();
    let assignments = peer.materialized.assignments.len();
    assert_eq!(admit(&mut peer, vec![bytes]).unwrap(), 0);
    assert_eq!(peer.materialized.cursor, cursor);
    assert_eq!(peer.materialized.assignments.len(), assignments);
    assert_eq!(peer.get("title"), source.get("title"));
    // Relay after older proofs have been pruned, without the original signing key.
    let relay = inventory(&peer)
        .into_iter()
        .find(|item| item.metadata.id() == id)
        .unwrap();
    let mut fresh = MvRegisterMap::new();
    fresh.set_write_permissions(source.view_point().write_permissions());
    admit(&mut fresh, vec![relay.bytes]).unwrap();
    assert_eq!(fresh.get("title"), source.get("title"));
}

#[test]
fn missing_dependencies_are_queued_and_duplicates_are_harmless() {
    let mut source = MvRegisterMap::new();
    let root = set_loose(&mut source, "root");
    let child = set_loose(&mut source, "child");
    let items = inventory(&source);
    let root_bytes = items
        .iter()
        .find(|i| i.metadata.id() == ItemId::Commit(root))
        .unwrap()
        .bytes
        .clone();
    let child_bytes = items
        .iter()
        .find(|i| i.metadata.id() == ItemId::Commit(child))
        .unwrap()
        .bytes
        .clone();
    let mut peer = MvRegisterMap::new();
    peer.set_write_permissions(source.view_point().write_permissions());
    assert_eq!(admit(&mut peer, vec![child_bytes.clone()]).unwrap(), 1);
    assert_eq!(peer.graph.pending_items(), 1);
    assert!(!peer.graph.has_commit(&child));
    assert!(peer.get("title").is_empty());
    // Only deliver the parent: the graph already owns the child's payload.
    assert_eq!(admit(&mut peer, vec![root_bytes.clone()]).unwrap(), 0);
    assert!(peer.graph.has_commit(&root));
    assert!(peer.graph.has_commit(&child));
    let cursor = peer.materialized.cursor.clone();
    for _ in 0..2 {
        assert_eq!(
            admit(
                &mut peer,
                vec![child_bytes.clone(), root_bytes.clone(), child_bytes.clone(),]
            )
            .unwrap(),
            0
        );
        assert_eq!(peer.materialized.assignments.len(), 2);
        assert_eq!(peer.materialized.cursor, cursor);
        assert_eq!(peer.get("title"), source.get("title"));
    }
}

#[test]
fn encoded_batch_failure_still_materializes_earlier_valid_items() {
    let mut source = MvRegisterMap::new();
    let root = set_loose(&mut source, "root");
    let bytes = inventory(&source).remove(0).bytes;
    let mut peer = MvRegisterMap::new();
    peer.set_write_permissions(source.view_point().write_permissions());
    assert!(admit(&mut peer, vec![bytes, vec![0]]).is_err());
    assert!(peer.graph.has_commit(&root));
    assert_eq!(peer.get("title"), source.get("title"));
}

#[test]
fn merge_failure_keeps_partial_report_and_materializes_earlier_admissions() {
    let mut source = MvRegisterMap::new();
    let root = (0..100_000)
        .find_map(|nonce| {
            let value = format!("root-{nonce}");
            let assignment = Assignment {
                key: "title".into(),
                value: value.clone(),
            };
            let meta = source
                .graph
                .describe_commit(source.author(), &assignment)
                .unwrap();
            // Leave at least half the hash space for the larger child hash.
            (1..128)
                .contains(&meta.hash().as_bytes()[0])
                .then(|| source.set("title", &value).unwrap())
        })
        .expect("loose root fixture");
    let child = (0..100_000)
        .find_map(|nonce| {
            let value = format!("{}-{nonce}", "x".repeat(100));
            let assignment = Assignment {
                key: "title".into(),
                value: value.clone(),
            };
            let meta = source
                .graph
                .describe_commit(source.author(), &assignment)
                .unwrap();
            // Both topological and hash-sorted export orders put the root first.
            (meta.hash() > &root && meta.hash().as_bytes()[0] != 0)
                .then(|| source.set("title", &value).unwrap())
        })
        .expect("loose child fixture");
    let (cover, _) = source
        .graph
        .export_changes(&ExportCursor::default())
        .unwrap();
    assert_eq!(
        cover
            .added
            .iter()
            .map(|item| item.metadata.id())
            .collect::<Vec<_>>(),
        vec![ItemId::Commit(root), ItemId::Commit(child)]
    );
    let mut peer = MvRegisterMap::new();
    peer.set_write_permissions(source.view_point().write_permissions());
    peer.graph.set_decode_limits(stockfoil::DecodeLimits {
        max_payload_bytes: 64,
        ..stockfoil::DecodeLimits::default()
    });
    let error = peer.merge(&source).unwrap_err();
    let partial = error
        .downcast_ref::<stockfoil::MergeError<std::io::Error>>()
        .unwrap();
    assert_eq!(partial.report.admitted, vec![ItemId::Commit(root)]);
    assert!(partial.report.rejected.is_empty());
    assert_eq!(partial.report.pending, 0);
    assert!(std::error::Error::source(partial).is_some());
    assert!(peer.graph.has_commit(&root));
    assert!(!peer.graph.has_commit(&child));
    assert_eq!(
        peer.get("title"),
        BTreeSet::from([source
            .materialized
            .assignments
            .iter()
            .find(|(hash, _)| *hash == root)
            .unwrap()
            .1
            .value
            .clone()])
    );
    // Retrying with a larger budget completes the same materialization normally.
    peer.graph
        .set_decode_limits(stockfoil::DecodeLimits::default());
    peer.merge(&source).unwrap();
    assert_eq!(peer.get("title"), source.get("title"));
}

#[test]
fn deferred_rejection_still_materializes_the_valid_dependency() {
    use crate::codec::RegisterCodec;
    use stockfoil::{FragmentShape, MemberShape, ParentRef, PayloadCodec};

    // Test-only malformed v1 fixture: a signed checkpoint with its ancestor
    // omitted from a one-member fragment with an empty boundary. Its low-depth
    // parent passes stateless verification, but cannot belong to empty boundary
    // history. Production receipt still uses only encoded bytes.
    fn framed(body: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut len = u32::try_from(body.len()).unwrap();
        while len >= 128 {
            bytes.push((len as u8 & 127) | 128);
            len >>= 7;
        }
        bytes.push(len as u8);
        bytes.extend_from_slice(body);
        bytes
    }

    let mut source = MvRegisterMap::new();
    let root = set_loose(&mut source, "root");
    let root_bytes = inventory(&source).remove(0).bytes;
    add_checkpoint(&mut source);
    let item = inventory(&source).remove(0);
    let TransportMetadata::Fragment { range, .. } = item.metadata else {
        panic!("fragment fixture")
    };
    assert!(range.boundary.is_empty());
    let payload = &source
        .materialized
        .assignments
        .iter()
        .find(|(hash, _)| *hash == range.head)
        .unwrap()
        .1;
    let shape = FragmentShape {
        members: vec![MemberShape {
            author: source.author(),
            parents: vec![ParentRef::External(root)],
        }],
    };
    let mut malformed = b"STKF\x01\x01".to_vec();
    malformed.extend(framed(RegisterCodec.format_id().as_bytes()));
    malformed.extend(range.head.as_bytes());
    malformed.extend([0, 1, 1]); // empty boundary; one member and author
    malformed.extend(source.author().as_slice());
    malformed.push(1); // one external parent
    malformed.extend(root.as_bytes());
    for column in [&[0x7f, 0][..], &[0x7f, 1], &[1], &[], &[0x7f, 0]] {
        malformed.extend(framed(column));
    }
    malformed.push(1); // signer 0, target member 0
    malformed.extend(framed(&[0x7f, 0]));
    malformed.extend(framed(&[0x7f, 0]));
    use ed25519_dalek::Signer;
    malformed.extend(source.signing_key.sign(range.head.as_bytes()).to_bytes());
    malformed.extend(framed(
        &RegisterCodec.encode_fragment(&shape, &[payload]).unwrap(),
    ));

    let mut peer = MvRegisterMap::new();
    peer.set_write_permissions(source.view_point().write_permissions());
    assert_eq!(admit(&mut peer, vec![malformed]).unwrap(), 1);
    assert!(peer.materialized.assignments.is_empty());
    // The application surfaces the deferred rejection, but still materializes
    // the valid dependency admitted by the very same byte-ingestion call.
    assert!(admit(&mut peer, vec![root_bytes.clone()]).is_err());
    assert_eq!(peer.graph.pending_items(), 0);
    assert!(peer.graph.has_commit(&root));
    assert!(!peer.graph.has_commit(&range.head));
    assert_eq!(peer.materialized.assignments.len(), 1);
    assert_eq!(peer.get("title"), source.get("title"));
    assert_eq!(admit(&mut peer, vec![root_bytes]).unwrap(), 0);
    assert_eq!(peer.materialized.assignments.len(), 1);
}

#[tokio::test]
async fn batch_publication_preserves_checkpoint_prefixes_and_advances_cursor() {
    let mut server = transport::start_server().await.unwrap();
    let remote = RemoteServer {
        url: format!("ws://{}", server.address()),
        peer_id: server.peer_id(),
    };
    let mut replica = Replica::new(Client::connect(&remote).await.unwrap());
    let result: Result<()> = async {
        let id = SedimentreeId::new(rand::random());
        let mut doc = MvRegisterMap::new();
        add_checkpoint(&mut doc);
        add_checkpoint_at_depth(&mut doc, 2);
        set_loose(&mut doc, "tip");
        let (cover, _) = doc.graph.export_changes(&ExportCursor::default())?;
        let expected = cover
            .added
            .iter()
            .find_map(|item| match &item.metadata {
                TransportMetadata::Fragment { checkpoints, .. } => Some(checkpoints),
                _ => None,
            })
            .unwrap();
        assert!(!expected.is_empty(), "exercise nonempty interior prefixes");

        // Publish uses local batch storage; no sync call is needed to read it.
        replica.publish(id, &doc).await?;
        let fragments = Storage::<Sendable>::load_fragment_metas(&replica.client.storage, id).await?;
        assert_eq!(fragments.len(), 1);
        assert_eq!(
            fragments[0].checkpoints(),
            &expected
                .iter()
                .map(|c| Checkpoint::from_bytes(c.0))
                .collect(),
        );
        let before = replica.client.node.get_blobs(id).await?;
        assert_eq!(before.len(), 2, "one fragment and one loose tip");
        assert!(
            doc.graph
                .export_changes(&replica.published)?
                .0
                .added
                .is_empty()
        );
        replica.publish(id, &doc).await?;
        assert_eq!(
            replica.client.node.get_blobs(id).await?.len(),
            before.len()
        );
        Ok(())
    }
    .await;
    let result = transport::with_cleanup(result, replica.client.close().await);
    server.stop_and_drain().await;
    result.unwrap();
}

#[tokio::test]
async fn managed_storage_failure_does_not_advance_the_export_cursor() {
    let mut server = transport::start_server().await.unwrap();
    let remote = RemoteServer {
        url: format!("ws://{}", server.address()),
        peer_id: server.peer_id(),
    };
    let mut replica = Replica::new(Client::connect(&remote).await.unwrap());
    let result: Result<()> = async {
        let id = SedimentreeId::new(rand::random());
        let mut doc = MvRegisterMap::new();
        set_loose(&mut doc, "root");
        set_loose(&mut doc, "child");
        replica.client.close().await?;
        let error = replica.publish(id, &doc).await.unwrap_err();
        assert_eq!(
            error.downcast_ref::<subduction_tokio::Error>().unwrap().kind(),
            subduction_tokio::error::ErrorKind::Closed,
        );
        assert_eq!(doc.graph.export_changes(&replica.published)?.0.added.len(), 2);

        // Nothing was stored or acknowledged. Retry the complete inventory on a
        // new client, without resetting or manually repairing the export cursor.
        replica.client = Client::connect(&remote).await?;
        replica.publish(id, &doc).await?;
        assert_eq!(replica.client.node.get_blobs(id).await?.len(), 2);
        assert!(doc.graph.export_changes(&replica.published)?.0.added.is_empty());
        Ok(())
    }
    .await;
    let result = transport::with_cleanup(result, replica.client.close().await);
    server.stop_and_drain().await;
    result.unwrap();
}

#[tokio::test]
async fn rejects_an_unexpected_server_identity() {
    let mut server = transport::start_server().await.unwrap();
    let remote = RemoteServer {
        url: format!("ws://{}", server.address()),
        peer_id: subduction_core::peer::id::PeerId::new([0; 32]),
    };
    let result = Client::connect(&remote).await;
    server.stop_and_drain().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn real_websocket_server_converges_registers() {
    tokio::time::timeout(Duration::from_secs(60), run(None))
        .await
        .unwrap()
        .unwrap();
}

// These scenarios are useful regressions, but aren't needed in the runnable demo.
#[tokio::test]
async fn websocket_fragments_history_revocation_drafts_and_fresh_replica() {
    let mut server = transport::start_server().await.unwrap();
    let remote = RemoteServer {
        url: format!("ws://{}", server.address()),
        peer_id: server.peer_id(),
    };
    let mut replicas = Vec::new();
    let result: Result<()> = async {
        replicas.push(Replica::new(Client::connect(&remote).await?));
        replicas.push(Replica::new(Client::connect(&remote).await?));
        let id = SedimentreeId::new(rand::random());
        let mut docs = vec![MvRegisterMap::new(), MvRegisterMap::new()];
        let permissions: HashMap<_, _> = docs
            .iter()
            .map(|d| (d.author(), WritePermission::Unbounded))
            .collect();
        for doc in &mut docs {
            doc.set_write_permissions(&permissions);
        }
        docs[0].set("title", "before")?;
        add_checkpoint(&mut docs[0]);
        let base = docs[0].view_point();
        converge(id, &mut replicas, &mut docs, |docs| {
            docs[1].view_point().heads() == base.heads()
        })
        .await?;
        assert!(
            !Storage::<Sendable>::load_fragment_metas(&replicas[1].client.storage, id)
                .await?
                .is_empty()
        );
        assert_eq!(docs[1].get("title"), BTreeSet::from(["before".into()]));

        docs[0].set("title", "Hello")?;
        docs[1].set("title", "Howdy")?;
        let expected = BTreeSet::from(["Hello".into(), "Howdy".into()]);
        converge(id, &mut replicas, &mut docs, |docs| {
            docs.iter().all(|d| d.get("title") == expected)
        })
        .await?;
        assert_eq!(
            docs[1].get_at(&base, "title")?,
            BTreeSet::from(["before".into()])
        );

        let alice = docs[0].author();
        docs[0].set_write_permissions(&HashMap::from([(alice, WritePermission::Unbounded)]));
        assert_eq!(docs[0].get("title"), BTreeSet::from(["Hello".into()]));
        assert_eq!(docs[1].get("title"), expected);

        let draft = docs[0].prepare_set("private", "draft")?;
        replicas[0].publish(id, &docs[0]).await?;
        replicas[0].sync(id).await?;
        replicas[1].sync(id).await?;
        replicas[1].load(id, &mut docs[1]).await?;
        assert!(!docs[1].graph.has_commit(&draft));
        docs[0].publish()?;
        converge(id, &mut replicas, &mut docs, |docs| {
            docs[1].get("private") == BTreeSet::from(["draft".into()])
        })
        .await?;

        replicas.push(Replica::new(Client::connect(&remote).await?));
        let mut fresh = MvRegisterMap::new();
        fresh.set_write_permissions(&permissions);
        docs.push(fresh);
        converge(id, &mut replicas, &mut docs, |docs| {
            docs[2].get("title") == expected
                && docs[2].get("private") == BTreeSet::from(["draft".into()])
        })
        .await?;
        Ok(())
    }
    .await;
    let mut result = result;
    for replica in replicas {
        result = transport::with_cleanup(result, replica.client.close().await);
    }
    server.stop_and_drain().await;
    result.unwrap();
}
