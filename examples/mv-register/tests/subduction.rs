//! Application-owned nodes consume Stockfoil's optional Sedimentree exports.
#[path = "support/channel.rs"]
mod channel;

use channel::{ChannelConnector, channel_pair};
use eyre::{Result, ensure};
use futures::StreamExt;
use sedimentree_core::{blob::Blob, id::SedimentreeId};
use std::time::Duration;
use stockfoil::sedimentree::{Metadata, describe};
use stockfoil::{ExportCursor, Heads, WritePermission, WritePermissions};
use stockfoil_mv_register::{Assignment, MvRegisterMap, Received};
use subduction_core::{peer::id::PeerId, policy::open::OpenPolicy, storage::memory::MemoryStorage};
use subduction_crypto::signer::memory::MemorySigner;
use subduction_tokio::{
    CallTimeout, ExpectedPeer, Node, NodeHandle, StoredEvent, StoredItem, StoredUpdates,
};

const TREE: SedimentreeId = SedimentreeId::new([42; 32]);

fn node(seed: u8, storage: MemoryStorage) -> (Node, PeerId) {
    let key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
    let peer = PeerId::from(key.verifying_key());
    let node = Node::builder()
        .storage(storage)
        .policy(OpenPolicy)
        .signer(MemorySigner::from_bytes(&key.to_bytes()))
        .start()
        .unwrap();
    (node, peer)
}

async fn store(doc: &MvRegisterMap, node: &NodeHandle, cursor: &mut ExportCursor) -> Result<()> {
    let mut plan = doc.document().ledger().export(cursor);
    let mut commits = Vec::new();
    let mut fragments = Vec::new();
    for i in 0..plan.len() {
        let (item, bytes) = doc.document().export_item(&plan, i)?;
        let blob = Blob::new(bytes);
        match describe(TREE, &item, blob.meta())? {
            Metadata::Commit(commit) => commits.push((commit, blob)),
            Metadata::Fragment(fragment) => fragments.push((fragment, blob)),
        }
    }
    node.store_built_batch(TREE, commits, fragments).await?;
    for i in 0..plan.len() {
        plan.acknowledge(i)?;
    }
    *cursor = plan.cursor();
    Ok(())
}

async fn apply_until(
    doc: &mut MvRegisterMap,
    updates: &mut StoredUpdates,
    done: impl Fn(&StoredEvent) -> bool,
) -> Result<()> {
    loop {
        let event = updates
            .next()
            .await
            .ok_or_else(|| eyre::eyre!("stream ended"))??;
        if let StoredEvent::Item(item) = &event {
            let blob = match item {
                StoredItem::Commit(value) => value.blob(),
                StoredItem::Fragment(value) => value.blob(),
            };
            ensure!(
                !matches!(doc.receive(blob.as_slice())?, Received::Evicted { .. }),
                "queue evicted an item"
            );
        }
        if done(&event) {
            return Ok(());
        }
    }
}

async fn replay(doc: &mut MvRegisterMap, handle: &NodeHandle) -> Result<()> {
    let mut updates = handle.load_and_watch(TREE).await?;
    apply_until(doc, &mut updates, |event| {
        matches!(event, StoredEvent::StorageChecked)
    })
    .await?;
    updates.close();
    Ok(())
}

fn checkpoint_value(doc: &MvRegisterMap) -> String {
    let history = doc.document().history();
    let heads: Vec<_> = history.heads(Heads::Delivered).collect();
    (0u64..)
        .map(|i| format!("checkpoint {i}"))
        .find(|value| {
            let mut builder = history.begin_commit(doc.author(), heads.clone()).unwrap();
            builder.update(&Assignment::new("count", value.as_str()).encode());
            builder.finish().hash.as_bytes()[0] == 0
        })
        .unwrap()
}

#[tokio::test]
async fn exports_sync_between_application_owned_nodes() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let (alice_node, alice_peer) = node(11, MemoryStorage::default());
        let (bob_node, bob_peer) = node(12, MemoryStorage::default());
        let (alice_handle, bob_handle) = (alice_node.handle(), bob_node.handle());
        let mut alice = MvRegisterMap::new();
        let mut bob = MvRegisterMap::new();
        let writers = WritePermissions::from([
            (alice.author(), WritePermission::Unbounded),
            (bob.author(), WritePermission::Unbounded),
        ]);
        alice.set_write_permissions(writers.clone())?;
        bob.set_write_permissions(writers)?;
        for i in 0..5 {
            alice.set("count", &i.to_string())?;
        }
        let checkpoint = checkpoint_value(&alice);
        alice.set("count", &checkpoint)?;
        bob.set("title", "offline")?;
        let (mut alice_cursor, mut bob_cursor) = (ExportCursor::new(), ExportCursor::new());
        store(&alice, &alice_handle, &mut alice_cursor).await?;
        store(&bob, &bob_handle, &mut bob_cursor).await?;

        let (left, right) = channel_pair();
        let _connections = tokio::try_join!(
            alice_handle.connect(ChannelConnector::new([left]), ExpectedPeer::Known(bob_peer)),
            bob_handle.connect(
                ChannelConnector::new([right]),
                ExpectedPeer::Known(alice_peer)
            ),
        )?;
        let mut alice_updates = alice_handle.load_and_watch(TREE).await?;
        let mut bob_updates = bob_handle.load_and_watch(TREE).await?;
        tokio::try_join!(
            apply_until(
                &mut alice,
                &mut alice_updates,
                |e| matches!(e, StoredEvent::PeerSynced { peer_id } if *peer_id == bob_peer)
            ),
            apply_until(
                &mut bob,
                &mut bob_updates,
                |e| matches!(e, StoredEvent::PeerSynced { peer_id } if *peer_id == alice_peer)
            ),
        )?;
        assert_eq!(alice.values(), bob.values());
        assert!(bob.get("count").contains(&checkpoint));
        assert_eq!(alice.waiting(), 0);
        assert_eq!(bob.waiting(), 0);

        alice.set("title", "from alice")?;
        bob.set("title", "from bob")?;
        store(&alice, &alice_handle, &mut alice_cursor).await?;
        store(&bob, &bob_handle, &mut bob_cursor).await?;
        // Local store does not broadcast. Schedule exchanges explicitly.
        for (handle, peer) in [(&alice_handle, bob_peer), (&bob_handle, alice_peer)] {
            let report = handle
                .sync_with_peer(peer, TREE, true, CallTimeout::Default)
                .await?;
            ensure!(
                report.success && report.transport_errors.is_empty(),
                "sync: {report:?}"
            );
        }
        // A fresh scan gives a local replay boundary, unlike polling until idle.
        replay(&mut alice, &alice_handle).await?;
        replay(&mut bob, &bob_handle).await?;
        assert_eq!(alice.values(), bob.values());
        assert_eq!(alice.get("title").len(), 2);
        alice_updates.close();
        bob_updates.close();
        for node in [alice_node, bob_node] {
            let report = node.close().await;
            ensure!(
                !report.forced && report.failure.is_none(),
                "shutdown: {report:?}"
            );
        }
        Ok::<_, eyre::Report>(())
    })
    .await
    .unwrap()
    .unwrap();
}

#[tokio::test]
async fn stored_exports_replay_without_signing_local_drafts() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let storage = MemoryStorage::default();
        let (first, _) = node(21, storage.clone());
        let mut writer = MvRegisterMap::new();
        writer.set("title", "stored")?;
        store(&writer, &first.handle(), &mut ExportCursor::new()).await?;
        assert!(!first.close().await.forced);

        let (reopened, _) = node(22, storage);
        let mut reader = MvRegisterMap::new();
        reader.set_write_permissions(WritePermissions::from([
            (writer.author(), WritePermission::Unbounded),
            (reader.author(), WritePermission::Unbounded),
        ]))?;
        let draft = reader.prepare_set("draft", "local only")?;
        replay(&mut reader, &reopened.handle()).await?;
        assert_eq!(reader.get("title"), writer.get("title"));
        assert!(!reader.document().history().is_admitted(&draft));
        assert!(reader.document().drafts().any(|d| d.hash == draft));
        assert!(!reopened.close().await.forced);
        Ok::<_, eyre::Report>(())
    })
    .await
    .unwrap()
    .unwrap();
}
