//! Synchronize independent registers through a Subduction WebSocket server.
use std::{
    collections::{BTreeSet, HashMap},
    time::Duration,
};

use eyre::{Result, ensure};
use sedimentree_core::{blob::Blob, id::SedimentreeId, loose_commit::id::CommitId};
use stockfoil::{CommitHash, CommitMeta, ExportCursor, UnverifiedItem, WritePermission};
use subduction_core::timeout::call::CallTimeout;

pub use crate::transport::RemoteServer;
use crate::{
    MvRegisterMap,
    transport::{self, Client},
    wire::{self, Decoded},
};

struct Replica {
    client: Client,
    published: ExportCursor,
}

impl Replica {
    fn new(client: Client) -> Self {
        Self {
            client,
            published: ExportCursor::default(),
        }
    }

    async fn publish(&mut self, id: SedimentreeId, doc: &MvRegisterMap) -> Result<()> {
        let (changes, next) = doc.graph.export_changes(&self.published);
        for item in changes.added {
            let blob = Blob::new(wire::encode(doc, &item)?);
            match &item {
                UnverifiedItem::Commit { meta, .. } => {
                    let parents: BTreeSet<_> = meta.parents().iter().map(commit_id).collect();
                    ensure!(parents.len() <= 255, "too many transport parents");
                    self.client
                        .node
                        .add_commit(id, commit_id(meta.hash()), parents, blob)
                        .await?;
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
                        .await?;
                }
            }
        }
        // Advance only after the whole batch is stored. Superseded items can
        // remain in Subduction; retirement is not a request to delete history.
        self.published = next;
        Ok(())
    }

    async fn sync(&self, id: SedimentreeId) -> Result<()> {
        let (success, _, errors) = self
            .client
            .node
            .sync_with_peer(
                &self.client.server,
                id,
                true,
                CallTimeout::TimeoutMillis(5_000),
            )
            .await?;
        ensure!(
            success && errors.is_empty(),
            "sync failed (requires an open-policy server): {errors:?}"
        );
        Ok(())
    }

    async fn load(&self, id: SedimentreeId, doc: &mut MvRegisterMap) -> Result<usize> {
        // Subduction supplies local blobs; Stockfoil verifies their contents.
        // This example assumes peers advertise transport metadata correctly.
        let incoming = self
            .client
            .node
            .get_blobs(id)
            .await?
            .into_iter()
            .flatten()
            .map(|blob| wire::decode(blob.as_slice()))
            .collect::<Result<_>>()?;
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

fn admit(doc: &mut MvRegisterMap, incoming: Vec<Decoded>) -> Result<usize> {
    let result = (|| {
        for decoded in incoming {
            let report = doc
                .graph
                .ingest(decoded.item, |meta| decoded.payloads[meta.hash()].clone())?;
            ensure!(
                report.rejected.is_empty(),
                "queued items rejected: {:?}",
                report.rejected
            );
        }
        Ok(doc.graph.pending_items())
    })();
    // Even on error, materialize any valid items admitted before it.
    doc.materialize();
    result
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
            // A sync response can precede completion of background ingestion.
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?
}

/// Synchronize two registers through a server. With `None`, start an in-memory
/// loopback server; otherwise connect to the supplied open-policy server.
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
    let mut replicas = Vec::new();
    let result = async {
        replicas.push(Replica::new(Client::connect(&remote).await?));
        replicas.push(Replica::new(Client::connect(&remote).await?));
        let id = SedimentreeId::new(rand::random());
        let mut docs = vec![MvRegisterMap::new(), MvRegisterMap::new()];
        // Author permissions are application configuration, not network data.
        let permissions: HashMap<_, _> = docs
            .iter()
            .map(|d| (d.author(), WritePermission::Unbounded))
            .collect();
        for doc in &mut docs {
            doc.set_write_permissions(&permissions);
        }

        docs[0].set("title", "before");
        converge(id, &mut replicas, &mut docs, |docs| {
            docs[0].get("title") == docs[1].get("title")
        })
        .await?;

        // Both edits start from the same history, before seeing the other edit.
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
        Ok(())
    }
    .await;
    // Close connections on both success and failure.
    for replica in replicas {
        replica.client.close().await;
    }
    if let Some(server) = &mut local_server {
        server.stop_and_drain().await;
    }
    result
}

/// Line-oriented control protocol for the JavaScript interoperability test.
pub mod interop;

#[cfg(test)]
mod tests;
