//! In-process transport for Subduction integration tests only.
use future_form::Sendable;
use futures::future::BoxFuture;
use std::collections::VecDeque;
use std::io;
use std::sync::Mutex;
use subduction_core::{
    handshake::{self, Handshake},
    timestamp::TimestampSeconds,
    transport::Transport,
};
use subduction_crypto::nonce::Nonce;
use subduction_tokio::{
    AttemptScope, AuthenticationContext, Connector, Error as TokioError, ErrorKind,
    EstablishedConnection,
};

/// An in-memory full-duplex byte channel for [`subduction_tokio`] connections.
#[derive(Clone, Debug)]
pub struct Channel {
    tx: async_channel::Sender<Vec<u8>>,
    rx: async_channel::Receiver<Vec<u8>>,
}

impl PartialEq for Channel {
    fn eq(&self, other: &Self) -> bool {
        self.tx.same_channel(&other.tx)
    }
}

impl Transport<Sendable> for Channel {
    type SendError = io::Error;
    type RecvError = io::Error;
    type DisconnectionError = io::Error;

    fn send_bytes(&self, bytes: &[u8]) -> BoxFuture<'_, Result<(), io::Error>> {
        let bytes = bytes.to_vec();
        Box::pin(async move { self.tx.send(bytes).await.map_err(io::Error::other) })
    }

    fn recv_bytes(&self) -> BoxFuture<'_, Result<Vec<u8>, io::Error>> {
        Box::pin(async move { self.rx.recv().await.map_err(io::Error::other) })
    }

    fn disconnect(&self) -> BoxFuture<'_, Result<(), io::Error>> {
        Box::pin(async move {
            self.tx.close();
            self.rx.close();
            Ok(())
        })
    }
}

impl Handshake<Sendable> for Channel {
    type Error = io::Error;

    fn send(&mut self, bytes: Vec<u8>) -> BoxFuture<'_, Result<(), io::Error>> {
        let tx = self.tx.clone();
        Box::pin(async move { tx.send(bytes).await.map_err(io::Error::other) })
    }

    fn recv(&mut self) -> BoxFuture<'_, Result<Vec<u8>, io::Error>> {
        let rx = self.rx.clone();
        Box::pin(async move { rx.recv().await.map_err(io::Error::other) })
    }
}

/// Create a connected pair of in-memory [`Channel`] endpoints.
pub fn channel_pair() -> (Channel, Channel) {
    let (tx1, rx1) = async_channel::bounded(64);
    let (tx2, rx2) = async_channel::bounded(64);
    (Channel { tx: tx1, rx: rx2 }, Channel { tx: tx2, rx: rx1 })
}

/// A [`subduction_tokio::Connector`] backed by one or more [`Channel`]s,
/// performing a real Ed25519 Subduction handshake on each attempt.
pub struct ChannelConnector {
    channels: Mutex<VecDeque<Channel>>,
}

impl ChannelConnector {
    pub fn new(channels: impl IntoIterator<Item = Channel>) -> Self {
        Self {
            channels: Mutex::new(channels.into_iter().collect()),
        }
    }
}

impl Connector for ChannelConnector {
    fn establish(
        &self,
        attempt: AttemptScope,
        authentication: AuthenticationContext,
    ) -> BoxFuture<'_, Result<EstablishedConnection, TokioError>> {
        Box::pin(async move {
            let channel = self
                .channels
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| TokioError::new(ErrorKind::Transport, "no more channels"))?;
            let (authenticated, ()) = handshake::initiate::<Sendable, _, _, _, _>(
                channel,
                |c, _| (c, ()),
                authentication.signer(),
                authentication.audience(),
                TimestampSeconds::new(1),
                Nonce::random(),
            )
            .await
            .map_err(|e| TokioError::with_source(ErrorKind::Authentication, "handshake", e))?;
            attempt.established(authenticated)
        })
    }
}
