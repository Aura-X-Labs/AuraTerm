//! Loopback russh server helpers shared by the SSH tests.
//!
//! Each test supplies its own `server::Handler` describing the server behaviour
//! it needs (a stalled channel open, a partial-success auth chain, ...); these
//! helpers only take care of the listener, the host key and the client side.

use std::net::SocketAddr;
use std::sync::Arc;

use russh::{client, server};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use super::{forwarding, known_hosts};

/// Serve exactly one connection with `handler` on an ephemeral loopback port.
/// The task ends when that connection does.
pub(super) async fn spawn_server<H>(handler: H) -> (SocketAddr, JoinHandle<()>)
where
    H: server::Handler + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let key = russh::keys::PrivateKey::random(
        &mut russh::keys::key::safe_rng(),
        russh::keys::Algorithm::Ed25519,
    )
    .unwrap();
    let config = Arc::new(server::Config {
        keys: vec![key],
        ..Default::default()
    });
    let task = tokio::spawn(async move {
        let Ok((stream, _)) = listener.accept().await else { return };
        if let Ok(session) = server::run_stream(config, stream, handler).await {
            let _ = session.await;
        }
    });
    (address, task)
}

/// Handshake with the loopback server, without authenticating.
pub(super) async fn connect(address: SocketAddr) -> client::Handle<known_hosts::ClientHandler> {
    let handler = known_hosts::ClientHandler::new(
        None,
        Arc::new(std::sync::Mutex::new(None)),
        forwarding::new_remote_forward_registry(),
        false,
    );
    client::connect(Arc::new(client::Config::default()), address, handler)
        .await
        .unwrap()
}
