//! Real WebSocket transport and ownership of its background tasks.

use std::{
    fmt::Debug,
    future::IntoFuture,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use eyre::{Result, WrapErr, eyre};
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
    runtime::Handle,
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
    TokioWebSocketServer::setup(
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
    .await
    .wrap_err("starting WebSocket server")
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
    lifecycle: Option<Lifecycle>,
    runtime: Handle,
}

impl Client {
    pub(crate) async fn connect(remote: &RemoteServer) -> Result<Self> {
        let deadline = Instant::now() + CONNECT_TIMEOUT;
        let signer = MemorySigner::generate();
        let (conn, ws_listener, ws_sender, ws_keepalive) = timeout_at(
            deadline,
            TokioWebSocketClient::with_max_message_size(
                remote.url.parse().wrap_err("invalid WebSocket URL")?,
                signer.clone(),
                Audience::known(remote.peer_id),
                MAX_MESSAGE_SIZE,
            ),
        )
        .await
        .wrap_err("WebSocket connection/handshake timed out")?
        .wrap_err("WebSocket connection/handshake failed")?;

        // Audience::known is not itself a check of the returned identity.
        // No tasks have been spawned yet; dropping these futures closes the
        // unadmitted transport on an identity mismatch.
        if conn.peer_id() != remote.peer_id {
            return Err(eyre!(
                "WebSocket peer identity mismatch: expected {}, received {}",
                remote.peer_id,
                conn.peer_id()
            ));
        }

        let tracker = TaskTracker::new();
        let (node, _handler, listener, manager) = SubductionBuilder::new()
            .signer(signer)
            .storage(MemoryStorage::default(), Arc::new(OpenPolicy))
            .spawner(TrackedTokioSpawn::new(tracker.clone()))
            .timer(TimeoutTokio)
            .nonce_cache(NonceCache::default())
            .depth_metric(CountLeadingZeroBytes)
            .build::<Sendable, Connection>();

        let mut lifecycle = Lifecycle {
            node: node.clone(),
            stopping: Arc::new(AtomicBool::new(false)),
            tracker,
            tasks: Vec::with_capacity(5),
        };
        // The builder listener already calls node.listen(): spawn it only once.
        lifecycle.spawn("Subduction listener", listener, false);
        lifecycle.spawn("Subduction manager", manager, false);
        lifecycle.spawn("WebSocket listener", ws_listener, true);
        lifecycle.spawn("WebSocket sender", ws_sender, true);
        lifecycle.spawn("WebSocket keepalive", ws_keepalive, true);
        let client = Self {
            node,
            server: remote.peer_id,
            lifecycle: Some(lifecycle),
            runtime: Handle::current(),
        };

        let admission = timeout_at(
            deadline,
            client.node.add_connection(conn.map(MessageTransport::new)),
        )
        .await
        .wrap_err("WebSocket admission timed out")
        .and_then(|result| result.wrap_err("WebSocket admission failed"));
        match admission {
            Ok(true) => Ok(client),
            result => {
                client.close().await;
                match result {
                    Err(error) => Err(error),
                    _ => Err(eyre!("fresh WebSocket connection was already registered")),
                }
            }
        }
    }

    pub(crate) async fn close(mut self) {
        if let Some(lifecycle) = self.lifecycle.take() {
            // Detach-safe: cancelling close must not interrupt the manager's
            // normal shutdown cleanup. The drain itself has bounded waits.
            report_join(
                "WebSocket client cleanup",
                self.runtime.spawn(lifecycle.drain()).await,
            );
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(lifecycle) = self.lifecycle.take() {
            lifecycle.shutdown();
            // Never block_on from Drop (which may run inside a Tokio task).
            // If the runtime is shutting down, Lifecycle's Drop aborts leftovers.
            self.runtime.spawn(lifecycle.drain());
        }
    }
}

struct Task {
    name: &'static str,
    handle: JoinHandle<()>,
    transport: bool,
}

struct Lifecycle {
    node: Arc<Node>,
    stopping: Arc<AtomicBool>,
    tracker: TaskTracker,
    tasks: Vec<Task>,
}

impl Lifecycle {
    fn spawn<F>(&mut self, name: &'static str, future: F, transport: bool)
    where
        F: IntoFuture,
        F::IntoFuture: Send + 'static,
        F::Output: Debug + Send + 'static,
    {
        let future = future.into_future();
        let stopping = self.stopping.clone();
        let node = self.node.clone();
        let handle = tokio::spawn(async move {
            let outcome = future.await;
            if !stopping.load(Ordering::Acquire) {
                eprintln!("{name} exited outside shutdown: {outcome:?}");
                node.shutdown();
            }
        });
        self.tasks.push(Task {
            name,
            handle,
            transport,
        });
    }

    fn shutdown(&self) {
        self.stopping.store(true, Ordering::Release);
        self.node.shutdown();
        self.tracker.close();
    }

    async fn drain(mut self) {
        self.shutdown();
        // This client transport's disconnect() is a no-op. Its WS tasks can
        // remain parked on socket IO, so explicitly unpark them as in e2e.rs.
        // Crucially, leave the manager alive to abort its connection loops.
        for task in &self.tasks {
            if task.transport {
                task.handle.abort();
            }
        }

        let mut joined = 0;
        let graceful = timeout(DRAIN_TIMEOUT, async {
            for task in &mut self.tasks {
                report_join(task.name, (&mut task.handle).await);
                joined += 1;
            }
            self.tracker.wait().await;
        })
        .await;

        if graceful.is_err() {
            eprintln!("WebSocket client shutdown timed out; aborting remaining tasks");
            for task in &self.tasks[joined..] {
                task.handle.abort();
            }
            if timeout(DRAIN_TIMEOUT, async {
                for task in &mut self.tasks[joined..] {
                    report_join(task.name, (&mut task.handle).await);
                }
                self.tracker.wait().await;
            })
            .await
            .is_err()
            {
                eprintln!(
                    "WebSocket client abort drain timed out ({} tracked tasks remain)",
                    self.tracker.len()
                );
            }
        }
    }
}

impl Drop for Lifecycle {
    fn drop(&mut self) {
        // Also covers cancellation of connect/close or runtime shutdown while
        // the best-effort asynchronous Drop cleanup is pending.
        self.shutdown();
        for task in &self.tasks {
            task.handle.abort();
        }
    }
}

fn report_join(name: &str, result: std::result::Result<(), tokio::task::JoinError>) {
    if let Err(error) = result
        && !error.is_cancelled()
    {
        eprintln!("{name} failed: {error}");
    }
}
