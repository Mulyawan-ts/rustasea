//! Ephemeral-port axum server for browser tests (feature `browser`).
//!
//! [`ServerHandle`] binds `127.0.0.1:0` (the OS picks a free port), serves a
//! caller-supplied [`axum::Router`] on a background tokio task, and exposes the
//! resulting base URL. The router is built by the test itself — the harness
//! never depends on `rustasea-app`, so the crate DAG stays acyclic.

use std::net::SocketAddr;

use axum::Router;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use super::error::{BrowserError, BrowserResult};

/// A running app server bound to an ephemeral loopback port.
///
/// Dropping the handle aborts the background task; [`ServerHandle::shutdown`]
/// additionally awaits its termination.
pub struct ServerHandle {
    addr: SocketAddr,
    task: Option<JoinHandle<()>>,
}

impl ServerHandle {
    /// Bind an ephemeral loopback port and serve `router` in the background.
    ///
    /// # Errors
    ///
    /// Returns [`BrowserError::Server`] when the socket cannot be bound or its
    /// address cannot be read.
    pub async fn start(router: Router) -> BrowserResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|err| BrowserError::Server(format!("bind ephemeral port: {err}")))?;
        let addr = listener
            .local_addr()
            .map_err(|err| BrowserError::Server(format!("listener address: {err}")))?;
        let task = tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, router).await {
                eprintln!("browser test server stopped unexpectedly: {err}");
            }
        });
        Ok(Self {
            addr,
            task: Some(task),
        })
    }

    /// The server's base URL, e.g. `http://127.0.0.1:54321`.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The OS-assigned port the server listens on.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Abort the background server task and wait for it to stop.
    pub async fn shutdown(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for ServerHandle {
    /// Abort the background task when `shutdown` was not called.
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
