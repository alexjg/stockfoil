use super::*;
use crate::Assignment;
use stockfoil::ItemId;

fn set_loose(doc: &mut MvRegisterMap, label: &str) -> CommitHash {
    for nonce in 0..100_000 {
        let value = format!("{label}-{nonce}");
        let assignment = Assignment {
            key: "title".into(),
            value: value.clone(),
        };
        let meta = doc
            .graph
            .commit_by(doc.author(), &bcs::to_bytes(&assignment).unwrap());
        if meta.hash().as_bytes()[0] != 0 {
            return doc.set("title", &value);
        }
    }
    panic!("could not construct loose fixture")
}

// Force fragment coverage rather than relying on a randomly occurring checkpoint.
fn add_checkpoint(doc: &mut MvRegisterMap) {
    for nonce in 0..100_000u64 {
        let value = nonce.to_string();
        let assignment = Assignment {
            key: "checkpoint".into(),
            value: value.clone(),
        };
        let meta = doc
            .graph
            .commit_by(doc.author(), &bcs::to_bytes(&assignment).unwrap());
        if meta.hash().as_bytes()[0] == 0 {
            doc.set("checkpoint", &value);
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
    let range = source.graph.minimal_cover().fragments[0].range.clone();
    let item = source
        .graph
        .export(&ItemId::Fragment(range.clone()))
        .unwrap();
    let bytes = wire::encode(&source, &item).unwrap();
    let mut metadata_only_change = item.clone();
    let UnverifiedItem::Fragment { checkpoints, .. } = &mut metadata_only_change else {
        unreachable!()
    };
    checkpoints.insert(stockfoil::Checkpoint([255; 12]));
    assert_eq!(wire::encode(&source, &metadata_only_change).unwrap(), bytes);
    let mut peer = MvRegisterMap::new();
    peer.set_write_permissions(source.view_point().write_permissions());
    assert_eq!(
        admit(&mut peer, vec![wire::decode(&bytes).unwrap()]).unwrap(),
        0
    );
    let cursor = peer.materialized.cursor.clone();
    let assignments = peer.materialized.assignments.len();
    assert_eq!(
        admit(&mut peer, vec![wire::decode(&bytes).unwrap()]).unwrap(),
        0
    );
    assert_eq!(peer.materialized.cursor, cursor);
    assert_eq!(peer.materialized.assignments.len(), assignments);
    assert_eq!(peer.get("title"), source.get("title"));
    // Relay after older proofs have been pruned, without the original signing key.
    let relay = peer.graph.export(&ItemId::Fragment(range)).unwrap();
    wire::decode(&wire::encode(&peer, &relay).unwrap()).unwrap();
}

#[test]
fn missing_dependencies_are_queued_and_duplicates_are_harmless() {
    let mut source = MvRegisterMap::new();
    let root = set_loose(&mut source, "root");
    let child = set_loose(&mut source, "child");
    let root_bytes = wire::encode(
        &source,
        &source.graph.export(&ItemId::Commit(root)).unwrap(),
    )
    .unwrap();
    let child_bytes = wire::encode(
        &source,
        &source.graph.export(&ItemId::Commit(child)).unwrap(),
    )
    .unwrap();
    let mut peer = MvRegisterMap::new();
    peer.set_write_permissions(source.view_point().write_permissions());
    assert_eq!(
        admit(&mut peer, vec![wire::decode(&child_bytes).unwrap()]).unwrap(),
        1
    );
    assert_eq!(peer.graph.pending_items(), 1);
    assert!(!peer.graph.has_commit(&child));
    assert!(peer.get("title").is_empty());
    // Only deliver the parent: the graph already owns the child's payload.
    assert_eq!(
        admit(&mut peer, vec![wire::decode(&root_bytes).unwrap()]).unwrap(),
        0
    );
    assert!(peer.graph.has_commit(&root));
    assert!(peer.graph.has_commit(&child));
    let cursor = peer.materialized.cursor.clone();
    for _ in 0..2 {
        assert_eq!(
            admit(
                &mut peer,
                vec![
                    wire::decode(&child_bytes).unwrap(),
                    wire::decode(&root_bytes).unwrap(),
                    wire::decode(&child_bytes).unwrap(),
                ]
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
fn deferred_rejection_still_materializes_the_valid_dependency() {
    let mut source = MvRegisterMap::new();
    let root = set_loose(&mut source, "root");
    let root_bytes = wire::encode(
        &source,
        &source.graph.export(&ItemId::Commit(root)).unwrap(),
    )
    .unwrap();
    add_checkpoint(&mut source);
    let range = source.graph.minimal_cover().fragments[0].range.clone();
    let mut malformed = source
        .graph
        .export(&ItemId::Fragment(range.clone()))
        .unwrap();
    let UnverifiedItem::Fragment { commits, .. } = &mut malformed else {
        unreachable!()
    };
    // Stateless verification accepts the parent's hash as a dependency, but
    // admission rejects omitting that parent's in-range history.
    commits.retain(|meta| *meta.hash() == range.head);
    let malformed = wire::decode(&wire::encode(&source, &malformed).unwrap()).unwrap();
    let mut peer = MvRegisterMap::new();
    peer.set_write_permissions(source.view_point().write_permissions());
    assert_eq!(admit(&mut peer, vec![malformed]).unwrap(), 1);
    assert!(peer.get("title").is_empty());
    assert!(admit(&mut peer, vec![wire::decode(&root_bytes).unwrap()]).is_err());
    assert_eq!(peer.graph.pending_items(), 0);
    assert!(peer.graph.has_commit(&root));
    assert!(!peer.graph.has_commit(&range.head));
    assert_eq!(peer.materialized.assignments.len(), 1);
    assert_eq!(peer.get("title"), source.get("title"));
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
        docs[0].set("title", "before");
        add_checkpoint(&mut docs[0]);
        let base = docs[0].view_point();
        converge(id, &mut replicas, &mut docs, |docs| {
            docs[1].view_point().heads() == base.heads()
        })
        .await?;
        assert!(
            replicas[1]
                .client
                .node
                .get_fragments(id)
                .await
                .is_some_and(|fragments| !fragments.is_empty())
        );
        assert_eq!(docs[1].get("title"), BTreeSet::from(["before".into()]));

        docs[0].set("title", "Hello");
        docs[1].set("title", "Howdy");
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

        let draft = docs[0].prepare_set("private", "draft");
        replicas[0].publish(id, &docs[0]).await?;
        replicas[0].sync(id).await?;
        replicas[1].sync(id).await?;
        replicas[1].load(id, &mut docs[1]).await?;
        assert!(!docs[1].graph.has_commit(&draft));
        docs[0].publish();
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
    for replica in replicas {
        replica.client.close().await;
    }
    server.stop_and_drain().await;
    result.unwrap();
}
