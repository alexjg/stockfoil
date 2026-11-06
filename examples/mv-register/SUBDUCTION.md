# Syncing the register with Subduction

Stockfoil's optional `sedimentree` feature converts an exported item into a
Sedimentree `LooseCommit` or `Fragment`. The application owns the Subduction
node, storage, connections, and update loop; the register does not wrap them.

```toml
stockfoil = { version = "0.1", features = ["sedimentree"] }
sedimentree_core = { version = "0.17", features = ["std"] }
```

The Subduction dependencies used below are in this example's `[dev-dependencies]`.
The snippets are checked with `cargo test -p stockfoil-mv-register --doc`;
[`tests/subduction.rs`](tests/subduction.rs) also exercises two connected nodes.

## Export and store

`MvRegisterMap::set` commits and signs an assignment. Alternatively, accumulate
local drafts with `prepare_set`, then explicitly sign them with `publish`.
Neither operation performs I/O. Unsigned drafts are not exported.

For each exported item, encode the bytes, construct a blob, and describe it with
`stockfoil::sedimentree::describe`. Pass the resulting metadata and blob to your
node. Acknowledge the export only after local storage succeeds.

This complete example stores signed edits and replays them into a fresh register:

```rust
use futures::StreamExt;
use sedimentree_core::{blob::Blob, id::SedimentreeId};
use stockfoil::{ExportCursor, WritePermission, WritePermissions};
use stockfoil::sedimentree::{self, Metadata};
use stockfoil_mv_register::MvRegisterMap;
use subduction_core::{policy::open::OpenPolicy, storage::memory::MemoryStorage};
use subduction_crypto::signer::memory::MemorySigner;
use subduction_tokio::{Node, StoredEvent, StoredItem};

#[tokio::main(flavor = "current_thread")]
async fn main() -> eyre::Result<()> {
    // Application-owned configuration. This fixed key is for demonstration only;
    // the node identity need not be the register author's identity.
    let node = Node::builder()
        .storage(MemoryStorage::default())
        .policy(OpenPolicy)
        .signer(MemorySigner::from_bytes(&[7; 32]))
        .start()?;
    let handle = node.handle();
    let tree = SedimentreeId::new([42; 32]);

    let mut writer = MvRegisterMap::new();
    writer.set("title", "Hello")?;
    writer.set("body", "Stored through Subduction")?;
    let mut cursor = ExportCursor::new();
    let mut plan = writer.document().ledger().export(&cursor);
    for index in 0..plan.len() {
        let (item, bytes) = writer.document().export_item(&plan, index)?;
        let blob = Blob::new(bytes); // Encrypt before this step if required.
        match sedimentree::describe(tree, &item, blob.meta())? {
            Metadata::Commit(commit) => {
                handle.store_built_batch(tree, vec![(commit, blob)], vec![]).await?;
            }
            Metadata::Fragment(fragment) => {
                handle.store_built_batch(tree, vec![], vec![(fragment, blob)]).await?;
            }
        }
        plan.acknowledge(index)?;
        cursor = plan.cursor();
    }
    assert!(writer.document().ledger().export(&cursor).is_empty());

    let mut reader = MvRegisterMap::new();
    reader.set_write_permissions(WritePermissions::from([
        (writer.author(), WritePermission::Unbounded),
    ]))?;
    let mut updates = handle.load_and_watch(tree).await?;
    loop {
        let event = updates.next().await.ok_or_else(|| eyre::eyre!("stream ended"))??;
        match event {
            StoredEvent::Item(item) => {
                let blob = match &item {
                    StoredItem::Commit(value) => value.blob(),
                    StoredItem::Fragment(value) => value.blob(),
                };
                // Decrypt first if the stored blob is encrypted.
                reader.receive(blob.as_slice())?;
            }
            StoredEvent::StorageChecked => break,
            _ => {}
        }
    }
    assert_eq!(reader.values(), writer.values());
    updates.close();
    let report = node.close().await;
    eyre::ensure!(!report.forced && report.failure.is_none(), "shutdown: {report:?}");
    Ok(())
}
```

The cursor tracks **local storage**, not acknowledgement by remote peers. Keep a
cursor per destination and persist it according to that destination's durability
contract. For bulk storage, collect commits and fragments into two vectors, call
`store_built_batch` once, then acknowledge the batch after success. A storage error
or cancellation can leave partial writes; it does not imply rollback.

`describe` needs only the export structure and `BlobMeta`, not the blob bytes or
a live document. Out-of-core callers can use `Ledger::export_item`, stream their
encoding into storage, and then describe the resulting blob.

## Connections and synchronization

Use Subduction's `NodeHandle::connect` with your transport connector and expected
peer identity. Peers must agree on the document's tree ID and register codec.
Configure Stockfoil write permissions independently of Subduction's connection
and storage policies.

An active `load_and_watch(tree)` stream replays local storage, performs initial
sync on current and future connections, and subscribes to incoming updates.
Keep consuming `StoredEvent::Item` as above to apply received bytes to the
register. `StorageChecked` marks a completed local scan, not global convergence;
`PeerSynced` marks a load-initiated peer round, not remote durable acknowledgement.
Handle `RescanRequired` by consuming the following replay and `StorageChecked`.
Stockfoil tolerates repeated items and parks items whose dependencies are missing.
Applications should handle reported evictions and admission failures.

**`store_built_batch` stores locally; it does not broadcast subsequent edits.**
Your application must schedule explicit sync rounds to propagate those edits.
For an already connected peer:

```rust
use sedimentree_core::id::SedimentreeId;
use subduction_core::peer::id::PeerId;
use subduction_tokio::{CallTimeout, NodeHandle};

async fn sync(node: &NodeHandle, peer: PeerId, tree: SedimentreeId) -> eyre::Result<()> {
    let report = node.sync_with_peer(peer, tree, true, CallTimeout::Default).await?;
    eyre::ensure!(
        report.success && report.transport_errors.is_empty(),
        "sync did not complete successfully: {report:?}",
    );
    Ok(())
}
```

Drive the exchange as required on the participating nodes and keep processing
incoming streams. A successful sync report is not a barrier proving that your
register update loop has applied every resulting item. The integration test uses
a fresh local replay after explicit rounds to check the resulting register state.

## Scope

The feature supplies unsigned Sedimentree metadata only. Blob encoding,
encryption, Subduction signing, storage, network scheduling, and lifecycle remain
application responsibilities. On receipt, pass the plaintext blob to normal
Stockfoil admission; this feature adds no outer-metadata validation layer.

The demonstration uses `MemoryStorage`, `OpenPolicy`, and unencrypted blobs.
Choose appropriate storage and policies for your application. Close update
streams and the node before shutting down the runtime.
