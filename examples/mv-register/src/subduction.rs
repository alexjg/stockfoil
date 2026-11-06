//! Synchronizing an [`MvRegisterMap`] over [`subduction_tokio`] using
//! [`NodeHandle::load_and_watch`] and [`NodeHandle::store_built_batch`].
use crate::MvRegisterMap;
use eyre::Result;
use futures::{FutureExt, StreamExt};
use sedimentree_core::{blob::Blob, id::SedimentreeId};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use stockfoil::{Author, CommitHash, ExportCursor, ViewPoint, WritePermissions};
use stockfoil_subduction::{
    ChannelConnector, Metadata, channel_pair, check_receipt, export_batch, receive_stored,
};
use subduction_core::{peer::id::PeerId, policy::open::OpenPolicy, storage::memory::MemoryStorage};
use subduction_crypto::signer::memory::MemorySigner;
use subduction_tokio::{
    CallTimeout, CloseReport, ConnectionHandle, ExpectedPeer, Node, NodeHandle, PeerSyncReport,
    StoredEvent, StoredUpdates,
};

/// An [`MvRegisterMap`] paired with a [`subduction_tokio::Node`] and a live
/// [`NodeHandle::load_and_watch`] stream for one [`SedimentreeId`].
pub struct SubductionReplica {
    tree: SedimentreeId,
    doc: MvRegisterMap,
    node: Node,
    handle: NodeHandle,
    peer_id: PeerId,
    updates: StoredUpdates,
    cursor: ExportCursor,
    pending_metadata: HashMap<CommitHash, Metadata>,
}

impl SubductionReplica {
    /// Start a fresh replica and `subduction_tokio::Node` for `tree`.
    pub async fn new(tree: SedimentreeId) -> Result<Self> {
        Self::with_doc(tree, MvRegisterMap::new()).await
    }

    /// Start a `subduction_tokio::Node` for `doc` with fresh in-memory storage.
    pub async fn with_doc(tree: SedimentreeId, doc: MvRegisterMap) -> Result<Self> {
        Self::with_storage(tree, doc, MemoryStorage::default()).await
    }

    /// Start a `subduction_tokio::Node` for `doc` using `storage`, replay any
    /// existing items in `storage` through `load_and_watch`, and publish any
    /// admitted commits already in `doc`.
    pub async fn with_storage(
        tree: SedimentreeId,
        doc: MvRegisterMap,
        storage: MemoryStorage,
    ) -> Result<Self> {
        let signer = MemorySigner::from_bytes(&doc.signing_key().to_bytes());
        let peer_id = PeerId::from(doc.signing_key().verifying_key());
        let node = Node::builder()
            .storage(storage)
            .policy(OpenPolicy)
            .signer(signer)
            .start()?;
        let handle = node.handle();
        let updates = handle.load_and_watch(tree).await?;
        let mut replica = Self {
            tree,
            doc,
            node,
            handle,
            peer_id,
            updates,
            cursor: ExportCursor::new(),
            pending_metadata: HashMap::new(),
        };
        replica.wait_for_storage_checked().await?;
        replica.publish().await?;
        Ok(replica)
    }

    pub fn tree(&self) -> SedimentreeId {
        self.tree
    }

    pub fn author(&self) -> Author {
        self.doc.author()
    }

    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    pub fn handle(&self) -> &NodeHandle {
        &self.handle
    }

    pub fn doc(&self) -> &MvRegisterMap {
        &self.doc
    }

    pub fn doc_mut(&mut self) -> &mut MvRegisterMap {
        &mut self.doc
    }

    pub fn get(&self, key: &str) -> BTreeSet<String> {
        self.doc.get(key)
    }

    pub fn values(&self) -> &BTreeMap<String, BTreeSet<String>> {
        self.doc.values()
    }

    pub fn view_point(&self) -> ViewPoint {
        self.doc.view_point()
    }

    pub fn get_at(&self, view: &ViewPoint, key: &str) -> Result<BTreeSet<String>> {
        self.doc.get_at(view.heads(), key)
    }

    pub fn write_permissions(&self) -> &WritePermissions {
        self.doc.write_permissions()
    }

    pub fn set_write_permissions(&mut self, permissions: WritePermissions) -> Result<()> {
        self.doc.set_write_permissions(permissions)
    }

    /// Commit a local unsigned draft. Call [`Self::publish`] to sign and store
    /// it in Subduction.
    pub fn prepare_set(&mut self, key: &str, value: &str) -> Result<CommitHash> {
        self.doc.prepare_set(key, value)
    }

    /// Sign all local drafts and store any newly admitted items into the
    /// `subduction_tokio` node with `store_built_batch`.
    pub async fn publish(&mut self) -> Result<usize> {
        let outcome = self.doc.publish()?;
        for receipt in &outcome.released {
            check_receipt(
                self.doc.document().history(),
                &mut self.pending_metadata,
                receipt,
            )?;
        }
        let mut plan = self.doc.document().ledger().export(&self.cursor);
        let count = plan.len();
        if count > 0 {
            let (commits, fragments) =
                export_batch(self.doc.document(), &plan, self.tree, Blob::new)?;
            self.handle
                .store_built_batch(self.tree, commits, fragments)
                .await?;
            for index in 0..count {
                plan.acknowledge(index)?;
            }
            self.cursor = plan.cursor();
        }
        self.drain_updates().await?;
        Ok(count)
    }

    /// Commit, sign, and store `key = value` into the `subduction_tokio` node.
    pub async fn set(&mut self, key: &str, value: &str) -> Result<CommitHash> {
        let hash = self.prepare_set(key, value)?;
        self.publish().await?;
        Ok(hash)
    }

    /// Drain all currently ready events from the `load_and_watch` stream,
    /// admitting and checking any items they carry.
    pub async fn drain_updates(&mut self) -> Result<usize> {
        let mut items = 0;
        loop {
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
            let Some(next) = self.updates.next().now_or_never() else {
                break;
            };
            match next {
                Some(Ok(event)) => {
                    if self.apply_event(&event)? {
                        items += 1;
                    }
                }
                Some(Err(e)) => return Err(e.into()),
                None => break,
            }
        }
        Ok(items)
    }

    /// Await the next event on the `load_and_watch` stream and apply it.
    pub async fn next_event(&mut self) -> Result<Option<StoredEvent>> {
        let Some(event) = self.updates.next().await else {
            return Ok(None);
        };
        let event = event?;
        self.apply_event(&event)?;
        Ok(Some(event))
    }

    async fn wait_for_storage_checked(&mut self) -> Result<()> {
        while let Some(event) = self.next_event().await? {
            if matches!(event, StoredEvent::StorageChecked) {
                break;
            }
        }
        Ok(())
    }

    /// Wait until the `load_and_watch` stream reports `PeerSynced` for `peer`,
    /// applying all items delivered along the way.
    pub async fn wait_for_peer_sync(&mut self, peer: PeerId) -> Result<()> {
        while let Some(event) = self.next_event().await? {
            if matches!(event, StoredEvent::PeerSynced { peer_id } if peer_id == peer) {
                break;
            }
        }
        self.drain_updates().await?;
        Ok(())
    }

    fn apply_event(&mut self, event: &StoredEvent) -> Result<bool> {
        if let StoredEvent::Item(item) = event {
            receive_stored(self.doc.document_mut(), &mut self.pending_metadata, item)?;
            self.doc.materialize()?;
            let mut plan = self.doc.document().ledger().export(&self.cursor);
            let head = match item {
                subduction_tokio::StoredItem::Commit(v) => v.payload().head(),
                subduction_tokio::StoredItem::Fragment(v) => v.payload().head(),
            };
            for index in 0..plan.len() {
                if plan
                    .id(index)
                    .is_some_and(|id| id.head().as_bytes() == head.as_bytes())
                {
                    plan.acknowledge(index)?;
                }
            }
            self.cursor = plan.cursor();
            return Ok(true);
        }
        Ok(false)
    }

    /// Connect this replica's `subduction_tokio::Node` to `other`'s over an
    /// authenticated in-memory [`Channel`](stockfoil_subduction::Channel) pair,
    /// then wait for both `load_and_watch` streams to finish their initial peer sync.
    pub async fn connect(
        &mut self,
        other: &mut Self,
    ) -> Result<(ConnectionHandle, ConnectionHandle)> {
        let (left_ch, right_ch) = channel_pair();
        let left_peer = self.peer_id;
        let right_peer = other.peer_id;
        let left_connect = self.handle.connect(
            ChannelConnector::new([left_ch]),
            ExpectedPeer::Known(right_peer),
        );
        let right_connect = other.handle.connect(
            ChannelConnector::new([right_ch]),
            ExpectedPeer::Known(left_peer),
        );
        let (left_conn, right_conn) = tokio::try_join!(left_connect, right_connect)?;
        tokio::try_join!(
            self.wait_for_peer_sync(right_peer),
            other.wait_for_peer_sync(left_peer),
        )?;
        Ok((left_conn, right_conn))
    }

    /// Run a bidirectional Subduction sync round with `other` and drain both
    /// `load_and_watch` streams.
    pub async fn sync_with(&mut self, other: &mut Self) -> Result<PeerSyncReport> {
        let report = self
            .handle
            .sync_with_peer(other.peer_id, self.tree, true, CallTimeout::Default)
            .await?;
        let _ = other
            .handle
            .sync_with_peer(self.peer_id, self.tree, true, CallTimeout::Default)
            .await?;
        self.drain_updates().await?;
        other.drain_updates().await?;
        Ok(report)
    }

    /// Close the `load_and_watch` stream and shut down the `subduction_tokio::Node`.
    pub async fn close(mut self) -> CloseReport {
        self.updates.close();
        self.node.close().await
    }
}
