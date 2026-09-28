//! Real WebSocket transport and ownership of its background tasks.

use std::{future::IntoFuture, sync::Arc, time::Duration};

use eyre::{Result, ensure};
use future_form::Sendable;
use sedimentree_core::depth::CountLeadingZeroBytes;
use subduction_core::{
    handler::sync::SyncHandler,
    handshake::audience::Audience,
    nonce_cache::NonceCache,
    peer::id::PeerId,
    policy::open::OpenPolicy,
    storage::memory::MemoryStorage,
    subduction::{Subduction, builder::SubductionBuilder},
    transport::message::MessageTransport,
};
use subduction_crypto::signer::memory::MemorySigner;
use subduction_websocket::tokio::{
    TimeoutTokio, TrackedTokioSpawn, client::TokioWebSocketClient, server::TokioWebSocketServer,
};
use tokio::{
    task::JoinHandle,
    time::{Instant, timeout, timeout_at},
};
use tokio_util::task::TaskTracker;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// A server endpoint and the identity its handshake must prove.
#[derive(Clone, Debug)]
pub struct RemoteServer {
    pub url: String,
    pub peer_id: PeerId,
}

pub(crate) type Server = TokioWebSocketServer<
    MemoryStorage,
    OpenPolicy,
    MemorySigner,
    CountLeadingZeroBytes,
    TimeoutTokio,
>;

pub(crate) async fn start_server() -> Result<Server> {
    Ok(TokioWebSocketServer::setup(
        ([127, 0, 0, 1], 0).into(),
        TimeoutTokio,
        Duration::from_secs(60),
        MAX_MESSAGE_SIZE,
        MemorySigner::generate(),
        None,
        MemoryStorage::default(),
        OpenPolicy,
        NonceCache::default(),
        CountLeadingZeroBytes,
    )
    .await?)
}

type Connection = MessageTransport<TokioWebSocketClient<MemorySigner>>;
type Node = Subduction<
    'static,
    Sendable,
    MemoryStorage,
    Connection,
    SyncHandler<
        Sendable,
        MemoryStorage,
        Connection,
        OpenPolicy,
        CountLeadingZeroBytes,
        TrackedTokioSpawn,
    >,
    OpenPolicy,
    MemorySigner,
    TimeoutTokio,
    TrackedTokioSpawn,
>;

pub(crate) struct Client {
    pub node: Arc<Node>,
    pub server: PeerId,
    core: Vec<JoinHandle<()>>,
    websocket: Vec<JoinHandle<()>>,
    tracker: TaskTracker,
}

impl Client {
    pub(crate) async fn connect(remote: &RemoteServer) -> Result<Self> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let signer = MemorySigner::generate();
        let (conn, ws_listener, ws_sender, ws_keepalive) = timeout_at(
            deadline,
            TokioWebSocketClient::with_max_message_size(
                remote.url.parse()?,
                signer.clone(),
                Audience::known(remote.peer_id),
                MAX_MESSAGE_SIZE,
            ),
        )
        .await??;

        // Audience::known does not check the returned identity. Check before spawning.
        ensure!(
            conn.peer_id() == remote.peer_id,
            "WebSocket peer identity mismatch"
        );

        let tracker = TaskTracker::new();
        let (node, _handler, listener, manager) = SubductionBuilder::new()
            .signer(signer)
            .storage(MemoryStorage::default(), Arc::new(OpenPolicy))
            .spawner(TrackedTokioSpawn::new(tracker.clone()))
            .timer(TimeoutTokio)
            .nonce_cache(NonceCache::default())
            .depth_metric(CountLeadingZeroBytes)
            .build::<Sendable, Connection>();
        let client = Self {
            node,
            server: remote.peer_id,
            core: vec![spawn(listener), spawn(manager)],
            websocket: vec![spawn(ws_listener), spawn(ws_sender), spawn(ws_keepalive)],
            tracker,
        };
        ensure!(
            timeout_at(
                deadline,
                client.node.add_connection(conn.map(MessageTransport::new))
            )
            .await??,
            "WebSocket connection already registered"
        );
        Ok(client)
    }

    pub(crate) async fn close(mut self) {
        if let Some(drain) = self.shutdown() {
            let _ = drain.await;
        }
    }

    fn shutdown(&mut self) -> Option<JoinHandle<()>> {
        if self.core.is_empty() && self.websocket.is_empty() {
            return None;
        }
        self.node.shutdown();
        // Upstream disconnect() is a no-op: abort socket IO, but let the manager drain.
        for task in &self.websocket {
            task.abort();
        }
        self.tracker.close();
        let core = std::mem::take(&mut self.core);
        let websocket = std::mem::take(&mut self.websocket);
        let tracker = self.tracker.clone();
        // A separate task keeps cleanup running even if close() is cancelled.
        Some(tokio::spawn(async move {
            for mut task in core.into_iter().chain(websocket) {
                if timeout(DRAIN_TIMEOUT, &mut task).await.is_err() {
                    task.abort();
                    let _ = timeout(DRAIN_TIMEOUT, task).await;
                }
            }
            let _ = timeout(DRAIN_TIMEOUT, tracker.wait()).await;
        }))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // All clients live inside this example's Tokio runtime.
        self.shutdown();
    }
}

fn spawn<F>(future: F) -> JoinHandle<()>
where
    F: IntoFuture,
    F::IntoFuture: Send + 'static,
{
    let future = future.into_future();
    tokio::spawn(async move {
        let _ = future.await;
    })
}
