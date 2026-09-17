//! Idle (inter-chunk) timeout enforcement tests (GAP-030).
//!
//! These drive `HttpClient` against raw HTTP/1.1 servers that speak chunked
//! transfer encoding by hand, so a stalled body can be produced deterministically
//! without pulling a streaming server framework into the test tree.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use rustasea_http::{HttpClient, HttpError, TimeoutKind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Consume the request line + headers so the server can respond cleanly.
async fn drain_request(socket: &mut TcpStream) {
    let mut buffer = [0u8; 1024];
    let mut seen = Vec::new();
    loop {
        let read = socket.read(&mut buffer).await.unwrap_or(0);
        if read == 0 {
            break;
        }
        seen.extend_from_slice(&buffer[..read]);
        if seen.windows(4).any(|window| window == b"\r\n\r\n") || seen.len() > 8192 {
            break;
        }
    }
}

/// Write the chunked-response preamble (status line + headers).
async fn write_chunked_headers(socket: &mut TcpStream) {
    socket
        .write_all(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: text/plain\r\n\r\n",
        )
        .await
        .expect("write headers");
    socket.flush().await.expect("flush headers");
}

/// Server that sends headers and a first chunk, then stalls indefinitely.
async fn spawn_stalling_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("listener address");
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept connection");
        drain_request(&mut socket).await;
        write_chunked_headers(&mut socket).await;
        socket
            .write_all(b"5\r\nhello\r\n")
            .await
            .expect("write first chunk");
        socket.flush().await.expect("flush first chunk");
        // Never send the terminating chunk: the body stalls here.
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    addr
}

/// Server that streams three chunks faster than any reasonable idle budget,
/// then terminates the body.
async fn spawn_streaming_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("listener address");
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept connection");
        drain_request(&mut socket).await;
        write_chunked_headers(&mut socket).await;
        for part in ["hello", " ", "world"] {
            let chunk = format!("{:x}\r\n{}\r\n", part.len(), part);
            socket
                .write_all(chunk.as_bytes())
                .await
                .expect("write chunk");
            socket.flush().await.expect("flush chunk");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        socket
            .write_all(b"0\r\n\r\n")
            .await
            .expect("write terminator");
        socket.flush().await.expect("flush terminator");
    });
    addr
}

/// Server that accepts the connection but never sends a response.
async fn spawn_silent_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("listener address");
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept connection");
        drain_request(&mut socket).await;
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    addr
}

/// A stalled body aborts with the typed idle-timeout error, fast.
#[tokio::test]
async fn stalled_body_aborts_with_idle_timeout() {
    let addr = spawn_stalling_server().await;
    let client = HttpClient::new()
        .timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_millis(100));

    let started = Instant::now();
    let err = client
        .get(&format!("http://{addr}/stream"))
        .await
        .expect_err("stalled body must abort");
    let elapsed = started.elapsed();

    match err {
        HttpError::Timeout { kind } => assert_eq!(kind, TimeoutKind::Idle),
        other => panic!("expected HttpError::Timeout, got {other:?}"),
    }
    assert!(
        elapsed < Duration::from_secs(3),
        "idle timeout should fire well before the total budget, took {elapsed:?}"
    );
}

/// A body streaming faster than the idle budget completes successfully.
#[tokio::test]
async fn streaming_body_faster_than_idle_completes() {
    let addr = spawn_streaming_server().await;
    let client = HttpClient::new()
        .timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_millis(500));

    let response = client
        .get(&format!("http://{addr}/stream"))
        .await
        .expect("fast chunks must not trip the idle timeout");

    assert_eq!(response.status().as_u16(), 200);
    // The rebuild path must preserve the request URL.
    assert_eq!(response.url().path(), "/stream");
    let body = response.text().await.expect("read buffered body");
    assert_eq!(body, "hello world");
}

/// Without an idle timeout, the total budget still governs a silent server.
#[tokio::test]
async fn total_timeout_still_applies_without_idle() {
    let addr = spawn_silent_server().await;
    let client = HttpClient::new().timeout(Duration::from_millis(200));

    let err = client
        .get(&format!("http://{addr}/silent"))
        .await
        .expect_err("silent server must hit the total timeout");
    match err {
        HttpError::Timeout { kind } => assert_eq!(kind, TimeoutKind::Total),
        other => panic!("expected HttpError::Timeout, got {other:?}"),
    }
}
