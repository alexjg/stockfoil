//! Synchronize independent registers through an opaque Subduction server.
//! No in-memory register merge is used by this example.
use std::{
    collections::{BTreeSet, HashMap},
    time::Duration,
};

use eyre::{Result, WrapErr, ensure};
use sedimentree_core::{
    blob::Blob,
    fragment::{Fragment, checkpoint::Checkpoint},
    id::SedimentreeId,
    loose_commit::{LooseCommit, id::CommitId},
};
use stockfoil::{CommitHash, CommitMeta, ItemId, UnverifiedItem, WritePermission};
use subduction_core::timeout::call::CallTimeout;

pub use crate::transport::RemoteServer;
use crate::{
    Assignment, MvRegisterMap,
    transport::{self, Client},
    wire::{self, Decoded},
};

struct Replica {
    client: Client,
    published: BTreeSet<ItemId>,
    fragments_sent: usize,
}

impl Replica {
    fn new(client: Client) -> Self {
        Self {
            client,
            published: BTreeSet::new(),
            fragments_sent: 0,
        }
    }

    async fn publish(&mut self, id: SedimentreeId, doc: &MvRegisterMap) -> Result<()> {
        // Export the certified cover, not the structural summary or working heads.
        // In particular, pending local drafts never become transport items.
        let cover = doc.graph.minimal_cover();
        let items = cover
            .loose_commits
            .into_iter()
            .map(|c| ItemId::Commit(c.commit))
            .chain(
                cover
                    .fragments
                    .into_iter()
                    .map(|f| ItemId::Fragment(f.range)),
            );
        for id_to_send in items {
            if self.published.contains(&id_to_send) {
                continue;
            }
            let item = doc.graph.export(&id_to_send)?;
            let blob = Blob::new(wire::encode(doc, &item)?);
            match &item {
                UnverifiedItem::Commit { meta, .. } => {
                    let parents: BTreeSet<_> = meta.parents().iter().map(commit_id).collect();
                    ensure!(
                        parents.len() <= 255,
                        "Subduction permits at most 255 parents"
                    );
                    // A FragmentRequested hint does not authorize inventing a
                    // certificate. Publish only fragments from Stockfoil's cover.
                    self.client
                        .node
                        .add_commit(id, commit_id(meta.hash()), parents, blob)
                        .await
                        .map_err(|e| eyre::eyre!("publishing commit: {e:?}"))?;
                }
                UnverifiedItem::Fragment { range, commits, .. } => {
                    let checkpoints = checkpoint_ids(range.head, commits.iter());
                    ensure!(range.boundary.len() <= 255, "too many fragment boundaries");
                    ensure!(checkpoints.len() <= 65535, "too many fragment checkpoints");
                    self.client
                        .node
                        .add_fragment(
                            id,
                            commit_id(&range.head),
                            range.boundary.iter().map(commit_id).collect(),
                            &checkpoints,
                            blob,
                        )
                        .await
                        .map_err(|e| eyre::eyre!("publishing fragment: {e:?}"))?;
                    self.fragments_sent += 1;
                }
            }
            self.published.insert(id_to_send);
        }
        Ok(())
    }

    async fn sync(&self, id: SedimentreeId) -> Result<()> {
        // add_connection/add_commit alone do not establish a subscription.
        let (success, _, errors) = self
            .client
            .node
            .sync_with_peer(
                &self.client.server,
                id,
                true,
                CallTimeout::TimeoutMillis(5_000),
            )
            .await
            .map_err(|e| eyre::eyre!("sync request: {e:?}"))?;
        ensure!(
            success && errors.is_empty(),
            "sync failed (success={success}): {errors:?}; this demo needs an open-policy server (--auth open), not Keyhive delegation"
        );
        Ok(())
    }

    async fn load(&self, id: SedimentreeId, doc: &mut MvRegisterMap) -> Result<usize> {
        let mut incoming = Vec::new();
        for meta in self.client.node.get_commits(id).await.unwrap_or_default() {
            let digest = meta.blob_meta().digest();
            if let Some(blob) = self.client.node.get_blob(id, digest).await? {
                let decoded = wire::decode(blob.as_slice())?;
                validate_commit(id, &meta, &decoded)?;
                incoming.push(decoded);
            }
        }
        for meta in self.client.node.get_fragments(id).await.unwrap_or_default() {
            let digest = meta.summary().blob_meta().digest();
            if let Some(blob) = self.client.node.get_blob(id, digest).await? {
                let decoded = wire::decode(blob.as_slice())?;
                validate_fragment(id, &meta, &decoded)?;
                incoming.push(decoded);
            }
        }
        admit(doc, incoming)
    }
}

fn commit_id(hash: &CommitHash) -> CommitId {
    CommitId::new(*hash.as_bytes())
}

fn checkpoint_ids<'a>(
    head: CommitHash,
    commits: impl Iterator<Item = &'a CommitMeta>,
) -> Vec<CommitId> {
    commits
        .filter(|m| *m.hash() != head && m.hash().as_bytes()[0] == 0)
        .map(|m| commit_id(m.hash()))
        .collect()
}

// Bind the verified inner envelope to the metadata Subduction used for its DAG.
// A valid signature on unrelated inner data is not sufficient.
fn validate_commit(id: SedimentreeId, meta: &LooseCommit, decoded: &Decoded) -> Result<()> {
    ensure!(meta.sedimentree_id() == id, "wrong document");
    ensure!(
        *decoded.item.id() == ItemId::Commit(CommitHash::from(*meta.head().as_bytes())),
        "transport commit head does not match authenticated contents"
    );
    let inner = decoded
        .item
        .commits()
        .next()
        .expect("verified standalone commit");
    ensure!(
        *meta.parents() == inner.parents().iter().map(commit_id).collect(),
        "transport parents do not match authenticated contents"
    );
    Ok(())
}

fn validate_fragment(id: SedimentreeId, meta: &Fragment, decoded: &Decoded) -> Result<()> {
    ensure!(meta.sedimentree_id() == id, "wrong document");
    let ItemId::Fragment(range) = decoded.item.id() else {
        eyre::bail!("expected a fragment")
    };
    ensure!(
        commit_id(&range.head) == meta.head(),
        "fragment head mismatch"
    );
    ensure!(
        range
            .boundary
            .iter()
            .map(commit_id)
            .collect::<BTreeSet<_>>()
            == *meta.boundary(),
        "fragment boundary mismatch"
    );
    let checkpoints = checkpoint_ids(range.head, decoded.item.commits())
        .into_iter()
        .map(Checkpoint::new)
        .collect::<BTreeSet<_>>();
    ensure!(
        checkpoints == *meta.checkpoints(),
        "fragment checkpoint mismatch"
    );
    Ok(())
}

fn admit(doc: &mut MvRegisterMap, mut incoming: Vec<Decoded>) -> Result<usize> {
    // Subduction may deliver dependency items after their dependents. Retry from
    // its storage on the next poll; don't put a causal queue inside CommitGraph.
    // Stage admissions so a bad fragment cannot partially mutate this batch.
    let mut graph = doc.graph.clone();
    while let Some(index) = incoming.iter().position(|decoded| {
        decoded
            .item
            .dependencies()
            .all(|hash| graph.has_commit(hash))
    }) {
        let decoded = incoming.swap_remove(index);
        graph.ingest(decoded.item, |meta| decoded.payloads[meta.hash()].clone())?;
    }
    doc.graph = graph;
    // All network receipt uses the same materializer as edits and revocations.
    doc.materialize();
    Ok(incoming.len())
}

async fn converge(
    id: SedimentreeId,
    replicas: &mut [Replica],
    docs: &mut [MvRegisterMap],
    ready: impl Fn(&[MvRegisterMap]) -> bool,
) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            for (replica, doc) in replicas.iter_mut().zip(docs.iter()) {
                replica.publish(id, doc).await?;
            }
            for replica in replicas.iter() {
                replica.sync(id).await?;
            }
            let mut deferred = 0;
            for (replica, doc) in replicas.iter_mut().zip(docs.iter_mut()) {
                deferred += replica.load(id, doc).await?;
            }
            if deferred == 0 && ready(docs) {
                return Ok(());
            }
            // A successful request is not an acknowledgement that the server's
            // asynchronous ingestion and pushes have completed. Await actual data.
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .wrap_err("timed out waiting for authenticated document convergence")?
}

// Mine one tiny checkpoint fixture so the demo always exercises fragment sync,
// not just loose commits. Candidate payloads are not ingested until one is chosen.
fn add_checkpoint(doc: &mut MvRegisterMap) -> Result<()> {
    for nonce in 0..100_000u64 {
        let value = nonce.to_string();
        let assignment = Assignment {
            key: "checkpoint".into(),
            value: value.clone(),
        };
        let meta = doc
            .graph
            .commit_by(doc.author(), &bcs::to_bytes(&assignment)?);
        if meta.hash().as_bytes()[0] == 0 {
            doc.set("checkpoint", &value);
            return Ok(());
        }
    }
    eyre::bail!("could not mine the demo checkpoint")
}

/// Run a bounded two-replica demo, with a fresh third replica verifying server
/// catch-up. With `None`, start and stop an in-memory loopback Subduction server.
/// All synchronization uses real authenticated WebSocket connections.
pub async fn run(remote: Option<RemoteServer>) -> Result<()> {
    let mut local_server = if remote.is_none() {
        Some(transport::start_server().await?)
    } else {
        None
    };
    let remote = remote.unwrap_or_else(|| {
        let server = local_server.as_ref().unwrap();
        RemoteServer {
            url: format!("ws://{}", server.address()),
            peer_id: server.peer_id(),
        }
    });
    println!("Server: {} (peer {})", remote.url, remote.peer_id);
    let id = SedimentreeId::new(rand::random());
    println!("Document: {}", hex::encode(id.as_bytes()));
    let mut replicas = Vec::new();
    let result = async {
        replicas.push(Replica::new(Client::connect(&remote).await?));
        replicas.push(Replica::new(Client::connect(&remote).await?));
        let mut docs = vec![MvRegisterMap::new(), MvRegisterMap::new()];
        // Application trust is explicit, not learned from server payloads.
        let permissions: HashMap<_, _> = docs
            .iter()
            .map(|d| (d.author(), WritePermission::Unbounded))
            .collect();
        for doc in &mut docs {
            doc.set_write_permissions(&permissions);
        }
        docs[0].set("title", "before");
        add_checkpoint(&mut docs[0])?;
        let base = docs[0].view_point();
        converge(id, &mut replicas, &mut docs, |docs| {
            docs[1].view_point().heads() == base.heads()
        })
        .await?;
        ensure!(
            replicas[0].fragments_sent > 0,
            "demo must transfer a certified fragment"
        );
        ensure!(
            docs[1].get("title") == BTreeSet::from(["before".into()]),
            "initial sync failed"
        );

        // Both edits start from the shared history, before either sees the other.
        docs[0].set("title", "Hello");
        docs[1].set("title", "Howdy");
        let expected = BTreeSet::from(["Hello".into(), "Howdy".into()]);
        converge(id, &mut replicas, &mut docs, |docs| {
            docs.iter().all(|d| d.get("title") == expected)
        })
        .await?;
        println!(
            "Alice: {:?}\nBob:   {:?}",
            docs[0].get("title"),
            docs[1].get("title")
        );
        ensure!(
            docs[1].get_at(&base, "title")? == BTreeSet::from(["before".into()]),
            "historical view changed"
        );

        // The relay stores history, not our permission settings.
        let alice = docs[0].author();
        docs[0].set_write_permissions(&HashMap::from([(alice, WritePermission::Unbounded)]));
        ensure!(
            docs[0].get("title") == BTreeSet::from(["Hello".into()]),
            "revocation failed"
        );
        ensure!(
            docs[1].get("title") == expected,
            "permissions leaked across replicas"
        );
        println!("Alice after revoking Bob: {:?}", docs[0].get("title"));

        // Pending work must not leak to the server. Signing promotes it without
        // a second local materialization effect, then the peer receives it.
        let draft = docs[0].prepare_set("private", "draft");
        replicas[0].publish(id, &docs[0]).await?;
        replicas[0].sync(id).await?;
        replicas[1].sync(id).await?;
        replicas[1].load(id, &mut docs[1]).await?;
        ensure!(
            !docs[1].graph.has_commit(&draft),
            "unsigned draft escaped publication policy"
        );
        docs[0].publish();
        converge(id, &mut replicas, &mut docs, |docs| {
            docs[1].get("private") == BTreeSet::from(["draft".into()])
        })
        .await?;

        // A completely fresh document learns all history from the server, not
        // by cloning either register or calling its in-memory merge method.
        replicas.push(Replica::new(Client::connect(&remote).await?));
        let mut fresh = MvRegisterMap::new();
        fresh.set_write_permissions(&permissions);
        docs.push(fresh);
        converge(id, &mut replicas, &mut docs, |docs| {
            docs[2].get("title") == expected
                && docs[2].get("private") == BTreeSet::from(["draft".into()])
        })
        .await?;
        println!(
            "Fresh replica: {:?}; signed draft: {:?}",
            docs[2].get("title"),
            docs[2].get("private")
        );
        Ok(())
    }
    .await;
    // Drain on both success and failure; don't detach background socket tasks.
    for replica in replicas {
        replica.client.close().await;
    }
    if let Some(server) = &mut local_server {
        server.stop_and_drain().await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn fragment_payloads_verify_and_bind_to_transport_metadata() {
        let mut source = MvRegisterMap::new();
        set_loose(&mut source, "before");
        add_checkpoint(&mut source).unwrap();
        let id = SedimentreeId::new([3; 32]);
        let range = source.graph.minimal_cover().fragments[0].range.clone();
        let item = source
            .graph
            .export(&ItemId::Fragment(range.clone()))
            .unwrap();
        let bytes = wire::encode(&source, &item).unwrap();
        let decoded = wire::decode(&bytes).unwrap();
        let checkpoints = checkpoint_ids(range.head, decoded.item.commits());
        let blob = Blob::new(bytes);
        let outer = Fragment::new(
            id,
            commit_id(&range.head),
            range.boundary.iter().map(commit_id).collect(),
            &checkpoints,
            blob.meta(),
        );
        validate_fragment(id, &outer, &decoded).unwrap();
        assert!(validate_fragment(SedimentreeId::new([4; 32]), &outer, &decoded).is_err());
        let wrong = Fragment::new(
            id,
            CommitId::new([9; 32]),
            BTreeSet::new(),
            &[],
            blob.meta(),
        );
        assert!(validate_fragment(id, &wrong, &decoded).is_err());
        let wrong = Fragment::new(
            id,
            commit_id(&range.head),
            BTreeSet::from([CommitId::new([9; 32])]),
            &checkpoints,
            blob.meta(),
        );
        assert!(validate_fragment(id, &wrong, &decoded).is_err());
        let wrong = Fragment::new(
            id,
            commit_id(&range.head),
            BTreeSet::new(),
            &[CommitId::new([9; 32])],
            blob.meta(),
        );
        assert!(validate_fragment(id, &wrong, &decoded).is_err());
        let mut peer = MvRegisterMap::new();
        peer.set_write_permissions(source.view_point().write_permissions());
        let deferred = admit(&mut peer, vec![decoded]).unwrap();
        assert_eq!(deferred, 0);
        let cursor = peer.materialized.cursor.clone();
        let assignments = peer.materialized.assignments.len();
        assert_eq!(
            admit(&mut peer, vec![wire::decode(blob.as_slice()).unwrap()]).unwrap(),
            0
        );
        assert_eq!(peer.materialized.cursor, cursor);
        assert_eq!(peer.materialized.assignments.len(), assignments);
        assert_eq!(peer.get("title"), source.get("title"));
        // The peer can export the received certificate again after older proofs
        // have been pruned, without obtaining the original author's signing key.
        let relay = peer.graph.export(&ItemId::Fragment(range)).unwrap();
        wire::decode(&wire::encode(&peer, &relay).unwrap()).unwrap();
    }

    #[test]
    fn missing_dependencies_wait_outside_the_graph_and_transport_parents_are_checked() {
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
        let deferred = admit(&mut peer, vec![wire::decode(&child_bytes).unwrap()]).unwrap();
        assert_eq!(deferred, 1);
        assert!(!peer.graph.has_commit(&child));
        assert!(peer.get("title").is_empty());
        let deferred = admit(
            &mut peer,
            vec![
                wire::decode(&child_bytes).unwrap(),
                wire::decode(&root_bytes).unwrap(),
            ],
        )
        .unwrap();
        assert_eq!(deferred, 0);
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

        let id = SedimentreeId::new([5; 32]);
        let blob = Blob::new(child_bytes.clone());
        let decoded = wire::decode(&child_bytes).unwrap();
        let outer = LooseCommit::new(
            id,
            commit_id(&child),
            BTreeSet::from([commit_id(&root)]),
            blob.meta(),
        );
        validate_commit(id, &outer, &decoded).unwrap();
        let wrong = LooseCommit::new(id, commit_id(&child), BTreeSet::new(), blob.meta());
        assert!(validate_commit(id, &wrong, &decoded).is_err());
        let wrong = LooseCommit::new(
            id,
            commit_id(&root),
            BTreeSet::from([commit_id(&root)]),
            blob.meta(),
        );
        assert!(validate_commit(id, &wrong, &decoded).is_err());
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
}
