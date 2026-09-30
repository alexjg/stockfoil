//! Example configuration for managed clients and a loopback WebSocket server.

use std::time::Duration;

use eyre::{Result, ensure};
use sedimentree_core::depth::CountLeadingZeroBytes;
use subduction_core::{
    nonce_cache::NonceCache, peer::id::PeerId, policy::open::OpenPolicy,
    storage::memory::MemoryStorage,
};
use subduction_crypto::signer::memory::MemorySigner;
use subduction_tokio::{
    ClientHandle, CloseReport, ExpectedPeer, TokioClient, websocket::WebSocketConnector,
};
use subduction_websocket::tokio::{TimeoutTokio, server::TokioWebSocketServer};

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

/// Configuration only: Subduction owns connection attempts, tasks, and teardown.
pub(crate) struct Client {
    pub node: ClientHandle,
    pub server: PeerId,
    owner: TokioClient,
    // Tests inspect stored checkpoint metadata without a raw-core escape hatch.
    #[cfg(test)]
    pub storage: MemoryStorage,
}

impl Client {
    pub(crate) async fn connect(remote: &RemoteServer) -> Result<Self> {
        let storage = MemoryStorage::default();
        let owner = TokioClient::builder()
            .storage(storage.clone())
            .policy(OpenPolicy)
            .signer(MemorySigner::generate())
            .start()?;
        let node = owner.handle();
        // Reconnection is deliberately disabled. Dropping the connection handle
        // releases its observer, not the connection: the client owns its lifetime.
        if let Err(error) = node
            .connect(
                WebSocketConnector::new(&remote.url).max_message_size(MAX_MESSAGE_SIZE),
                ExpectedPeer::Known(remote.peer_id),
            )
            .await
        {
            return with_cleanup(Err(error.into()), check_close(owner.close().await));
        }
        Ok(Self {
            node,
            server: remote.peer_id,
            owner,
            #[cfg(test)]
            storage,
        })
    }

    pub(crate) async fn close(&self) -> Result<()> {
        check_close(self.owner.close().await)
    }
}

fn check_close(report: CloseReport) -> Result<()> {
    let context = format!(
        "Subduction shutdown: forced={}, unfinished_tasks={}",
        report.forced, report.unfinished_tasks,
    );
    if let Some(error) = report.failure {
        return Err(eyre::Report::new(error).wrap_err(context));
    }
    ensure!(!report.forced && report.unfinished_tasks == 0, context);
    Ok(())
}

/// Accumulate cleanup errors without skipping other clients/server teardown or
/// losing the original operation's error chain.
pub(crate) fn with_cleanup<T>(result: Result<T>, cleanup: Result<()>) -> Result<T> {
    match (result, cleanup) {
        (result, Ok(())) => result,
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => {
            Err(error.wrap_err(format!("cleanup also failed: {cleanup:#}")))
        }
    }
}

#[cfg(test)]
mod tests;
