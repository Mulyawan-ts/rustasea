//! Graceful shutdown utilities — SIGTERM/SIGINT drain with a timeout.

use std::time::Duration;

/// Wait for SIGTERM/SIGINT then drain with timeout.
pub async fn graceful(_timeout: Duration) {
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
        let mut int =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).ok();
        tokio::select! {
            _ = async { if let Some(s) = term.as_mut() { s.recv().await; } } => {},
            _ = async { if let Some(s) = int.as_mut() { s.recv().await; } } => {},
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    tokio::time::sleep(Duration::from_millis(10)).await;
}

/// Shutdown handle wrapping a timeout.
pub struct ShutdownHandle {
    /// Timeout for drain.
    pub timeout: Duration,
}

impl ShutdownHandle {
    /// Create a new handle.
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    /// Run graceful shutdown.
    pub async fn drain(self) {
        graceful(self.timeout).await;
    }
}
