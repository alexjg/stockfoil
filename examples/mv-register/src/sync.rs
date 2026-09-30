//! Synchronize independent registers through a Subduction WebSocket server.
use std::{
    collections::{BTreeSet, HashMap},
    time::Duration,
};

use eyre::{Result, ensure};
use sedimentree_core::{
    blob::Blob,
    fragment::{Fragment, checkpoint::Checkpoint},
    id::SedimentreeId,
    loose_commit::{LooseCommit, id::CommitId},
};
use stockfoil::{CommitHash, ExportCursor, TransportMetadata, WritePermission};
use subduction_tokio::CallTimeout;

pub use crate::transport::RemoteServer;
use crate::{
    MvRegisterMap, check_report,
    transport::{self, Client},
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
        let (changes, next) = doc.graph.export_changes(&self.published)?;
        let (mut commits, mut fragments) = (Vec::new(), Vec::new());
        for item in changes.added {
            let blob = Blob::new(item.bytes);
            match &item.metadata {
                TransportMetadata::Commit { hash, parents } => {
                    let parents: BTreeSet<_> = parents.iter().map(commit_id).collect();
                    commits.push((
                        LooseCommit::new(id, commit_id(hash), parents, blob.meta())?,
                        blob,
                    ));
                }
                TransportMetadata::Fragment { range, checkpoints } => {
                    fragments.push((
                        Fragment::from_parts(
                            id,
                            commit_id(&range.head),
                            range.boundary.iter().map(commit_id).collect(),
                            checkpoints
                                .iter()
                                .map(|c| Checkpoint::from_bytes(c.0))
                                .collect(),
                            blob.meta(),
                        )?,
                        blob,
                    ));
                }
            }
        }
        // Store once, without network I/O; sync() handles propagation separately.
        self.client
            .node
            .store_built_batch(id, commits, fragments)
            .await?;
        // Advance only after the whole batch is stored. Superseded items can
        // remain in Subduction; retirement is not a request to delete history.
        self.published = next;
        Ok(())
    }

    async fn sync(&self, id: SedimentreeId) -> Result<()> {
        let report = self
            .client
            .node
            .sync_with_peer(
                self.client.server,
                id,
                true,
                CallTimeout::TimeoutMillis(5_000),
            )
            .await?;
        ensure!(
            report.success && report.transport_errors.is_empty(),
            "sync failed (requires an open-policy server): {:?}",
            report.transport_errors
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
            .map(|blob| blob.as_slice().to_vec())
            .collect();
        admit(doc, incoming)
    }
}

fn commit_id(hash: &CommitHash) -> CommitId {
    CommitId::new(*hash.as_bytes())
}

fn admit(doc: &mut MvRegisterMap, incoming: Vec<Vec<u8>>) -> Result<usize> {
    let result = (|| {
        for bytes in incoming {
            let report = doc.graph.ingest(&bytes)?;
            check_report(&report)?;
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
    let mut result = async {
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

        docs[0].set("title", "before")?;
        converge(id, &mut replicas, &mut docs, |docs| {
            docs[0].get("title") == docs[1].get("title")
        })
        .await?;

        // Both edits start from the same history, before seeing the other edit.
        docs[0].set("title", "Hello")?;
        docs[1].set("title", "Howdy")?;
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
        result = transport::with_cleanup(result, replica.client.close().await);
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
