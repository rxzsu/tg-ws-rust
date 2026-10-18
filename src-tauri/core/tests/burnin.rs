//! Burn-in tests for the proxy server lifecycle.
//!
//! These are deliberately network-free: every failure path they assert on is
//! reached *before* any upstream connection is attempted, which makes them
//! deterministic and fast (the whole file runs in a couple of seconds).
//!
//! Covered cycle:
//!   1. server starts and listens on a real port;
//!   2. clients that must fail (garbage, wrong secret, plain HTTP on a Fake
//!      TLS domain) fail cleanly — telemetry records it, server survives;
//!   3. shutdown drains live sessions and releases the port;
//!   4. the server restarts on the same port.

use std::sync::Arc;
use std::time::Duration;

use tg_ws_proxy_core::config::{PROTO_TAG_ABRIDGED, ProxyConfig};
use tg_ws_proxy_core::stats::Stats;
use tg_ws_proxy_core::run_server_with_stats;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Probe a free port; the tiny race with a rebinding test is irrelevant here.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind probe");
    listener.local_addr().expect("probe addr").port()
}

/// Minimal config that never reaches the network: no pool warm-up, no CF.
fn test_config(port: u16) -> ProxyConfig {
    ProxyConfig {
        host: "127.0.0.1".into(),
        port,
        secret: "00112233445566778899aabbccddeeff".into(),
        fake_tls_domain: String::new(),
        pool_size: 0,
        fallback_cfproxy: false,
        quic_relays: Vec::new(),
        ..Default::default()
    }
}

/// Wait until the port accepts connections (server thread scheduled).
async fn wait_listening(port: u16) {
    for _ in 0..150 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("server never started listening on {}", port);
}

async fn connect(port: u16) -> tokio::net::TcpStream {
    tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to proxy")
}

/// Read until the peer closes (or timeout) and return what arrived.
async fn read_until_close(socket: &mut tokio::net::TcpStream) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        match tokio::time::timeout(Duration::from_millis(1500), socket.read(&mut buf)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => out.extend_from_slice(&buf[..n]),
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }
    out
}

/// A client-side obfuscated2 handshake: the inverse of `try_handshake`.
///
/// Bytes 8..56 are the prekey+iv in plaintext (the server reads them as-is),
/// and the last 8 bytes carry the proto tag / DC index XORed with the keystream
/// derived from `sha256(prekey[..32] || secret)` — exactly the mirror image of
/// `generate_relay_init` on the server side.
fn build_client_handshake(secret: &[u8], proto_tag: &[u8; 4], dc_idx: i16) -> [u8; 64] {
    use aes::cipher::{KeyIvInit, StreamCipher};
    use aes::Aes256;
    use ctr::Ctr128BE;
    use rand::RngExt;
    use sha2::{Digest, Sha256};

    let mut rng = rand::rng();
    let mut handshake = [0u8; 64];
    rng.fill(&mut handshake);

    // Random prekey + IV at 8..56, as the protocol expects.
    let mut prekey_and_iv = [0u8; 48];
    rng.fill(&mut prekey_and_iv);
    handshake[8..56].copy_from_slice(&prekey_and_iv);

    let mut hasher = Sha256::new();
    hasher.update(&prekey_and_iv[0..32]);
    hasher.update(secret);
    let dec_key: [u8; 32] = hasher.finalize().into();
    let mut dec_iv = [0u8; 16];
    dec_iv.copy_from_slice(&prekey_and_iv[32..48]);

    // Keystream of the whole packet (position based, data independent).
    let mut keystream = [0u8; 64];
    Ctr128BE::<Aes256>::new((&dec_key).into(), (&dec_iv).into())
        .apply_keystream(&mut keystream);

    let mut tail_plain = [0u8; 8];
    tail_plain[0..4].copy_from_slice(proto_tag);
    tail_plain[4..6].copy_from_slice(&dc_idx.to_le_bytes());
    rng.fill(&mut tail_plain[6..8]);

    for i in 0..8 {
        handshake[56 + i] = tail_plain[i] ^ keystream[56 + i];
    }

    // The first 8 bytes are the "skip" field — avoid values the router treats
    // as TLS/HTTP frames.
    loop {
        rng.fill(&mut handshake[0..8]);
        let start = &handshake[0..4];
        if start == b"HEAD" || start == b"POST" || start == b"GET " || start[0] == 0xef {
            continue;
        }
        break;
    }

    handshake
}

/// Core burn-in: garbage in, telemetry out, server stays up, stop drains,
/// port is released, server restarts on the same port.
#[tokio::test]
async fn start_garbage_restart_cycle() {
    let port = free_port();
    let stats = Arc::new(Stats::new());
    let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(1);

    let server = tokio::spawn(run_server_with_stats(
        test_config(port),
        stats.clone(),
        shutdown_rx,
    ));
    wait_listening(port).await;
    // The readiness probe counts as a client of its own, and its session fails
    // the moment the probe drops. Let that settle before taking the baseline,
    // otherwise the deltas below race with the probe's own failure.
    tokio::time::sleep(Duration::from_millis(120)).await;
    while stats.snapshot(0.0, 0.0).connections_active > 0 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let baseline = stats.snapshot(0.0, 0.0);

    // (1) Garbage client: 64 zero bytes, never a valid handshake.
    {
        let mut client = connect(port).await;
        client.write_all(&[0u8; 64]).await.unwrap();
        let _ = read_until_close(&mut client).await;
    }

    // Telemetry captured the failure instead of swallowing it.
    let after = stats.snapshot(0.0, 0.0);
    assert_eq!(
        after.connections_total - baseline.connections_total,
        1,
        "exactly one new client was counted"
    );
    assert_eq!(
        after.connections_bad - baseline.connections_bad,
        1,
        "garbage counted as bad"
    );
    assert_eq!(after.connections_active, 0, "session was released");
    assert!(
        !after.last_error.is_empty(),
        "the failure must be visible on the dashboard, got: {:?}",
        after.last_error
    );

    // (2) Server is still alive and accepts another client.
    {
        let mut client = connect(port).await;
        client.write_all(&[1u8; 64]).await.unwrap();
        let _ = read_until_close(&mut client).await;
    }
    assert_eq!(
        stats.snapshot(0.0, 0.0).connections_total - baseline.connections_total,
        2,
        "server accepted a client after a failed one"
    );

    // (3) Stop: the server task must finish (it only returns after draining
    // live sessions) and the port must become bindable again.
    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server must exit after shutdown, not hang")
        .unwrap()
        .expect("server must exit cleanly");

    let rebound = std::net::TcpListener::bind(("127.0.0.1", port))
        .expect("port must be released after shutdown");
    drop(rebound);

    // (4) Restart on the same port with the same stats.
    let (restart_tx, restart_rx) = tokio::sync::broadcast::channel(1);
    let server = tokio::spawn(run_server_with_stats(
        test_config(port),
        stats.clone(),
        restart_rx,
    ));
    wait_listening(port).await;
    // The readiness probe counts as a client of its own, and its session fails
    // the moment the probe drops. Let that settle before taking the baseline,
    // otherwise the deltas below race with the probe's own failure.
    tokio::time::sleep(Duration::from_millis(120)).await;
    while stats.snapshot(0.0, 0.0).connections_active > 0 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let baseline = stats.snapshot(0.0, 0.0);

    let mut client = connect(port).await;
    client.write_all(&[2u8; 64]).await.unwrap();
    let _ = read_until_close(&mut client).await;
    assert_eq!(
        stats.snapshot(0.0, 0.0).connections_total,
        baseline.connections_total + 1,
        "the restarted server keeps counting on the same stats"
    );

    restart_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("restarted server must exit")
        .unwrap()
        .unwrap();
}

/// An idle-but-open client must not keep the server from shutting down: the
/// drain window is what stops that, and it must always terminate.
#[tokio::test]
async fn open_session_does_not_block_shutdown() {
    let port = free_port();
    let stats = Arc::new(Stats::new());
    let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(1);

    let server = tokio::spawn(run_server_with_stats(
        test_config(port),
        stats.clone(),
        shutdown_rx,
    ));
    wait_listening(port).await;
    // The readiness probe counts as a client of its own, and its session fails
    // the moment the probe drops. Let that settle before taking the baseline,
    // otherwise the deltas below race with the probe's own failure.
    tokio::time::sleep(Duration::from_millis(120)).await;
    while stats.snapshot(0.0, 0.0).connections_active > 0 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let _ = stats.snapshot(0.0, 0.0); // settle: no stat assertions here

    // Open a session and leave it hanging with no data at all.
    let held = connect(port).await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let started = std::time::Instant::now();
    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("drain must not hang on an idle session")
        .unwrap()
        .unwrap();

    assert!(
        started.elapsed() < Duration::from_secs(5),
        "drain took {:?}",
        started.elapsed()
    );
    drop(held);
}

/// A handshake built for the wrong secret must be rejected, not silently
/// mapped onto a zero key (the old behaviour looked like a working proxy).
#[tokio::test]
async fn wrong_secret_is_rejected() {
    let port = free_port();
    let stats = Arc::new(Stats::new());
    let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(1);

    let server = tokio::spawn(run_server_with_stats(
        test_config(port),
        stats.clone(),
        shutdown_rx,
    ));
    wait_listening(port).await;
    // The readiness probe counts as a client of its own, and its session fails
    // the moment the probe drops. Let that settle before taking the baseline,
    // otherwise the deltas below race with the probe's own failure.
    tokio::time::sleep(Duration::from_millis(120)).await;
    while stats.snapshot(0.0, 0.0).connections_active > 0 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let baseline = stats.snapshot(0.0, 0.0);

    let wrong_secret = b"deadbeefdeadbeefdeadbeefdeadbeef";
    let handshake = build_client_handshake(wrong_secret, &PROTO_TAG_ABRIDGED, 2);

    let mut client = connect(port).await;
    client.write_all(&handshake).await.unwrap();
    let _ = read_until_close(&mut client).await;

    let after = stats.snapshot(0.0, 0.0);
    assert_eq!(
        after.connections_bad - baseline.connections_bad,
        1,
        "handshake must be rejected"
    );
    assert!(
        !after.last_error.is_empty(),
        "rejection must be reported to the UI"
    );

    // And the server is still usable afterwards.
    let mut probe = connect(port).await;
    probe.write_all(&[0x33; 64]).await.unwrap();
    let _ = read_until_close(&mut probe).await;

    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server must exit")
        .unwrap()
        .unwrap();
}

/// With a Fake TLS domain configured, a plain HTTP request gets a 301
/// redirect instead of a hanging connection.
#[tokio::test]
async fn fake_tls_domain_redirects_plain_http() {
    let port = free_port();
    let stats = Arc::new(Stats::new());
    let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(1);

    let mut cfg = test_config(port);
    cfg.fake_tls_domain = "example.org".into();

    let server = tokio::spawn(run_server_with_stats(cfg, stats.clone(), shutdown_rx));
    wait_listening(port).await;
    // The readiness probe counts as a client of its own, and its session fails
    // the moment the probe drops. Let that settle before taking the baseline,
    // otherwise the deltas below race with the probe's own failure.
    tokio::time::sleep(Duration::from_millis(120)).await;
    while stats.snapshot(0.0, 0.0).connections_active > 0 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let _ = stats.snapshot(0.0, 0.0); // settle: no stat assertions here

    let mut client = connect(port).await;
    client.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
    let response = read_until_close(&mut client).await;

    let text = String::from_utf8_lossy(&response);
    assert!(
        text.starts_with("HTTP/1.1 301 Moved Permanently"),
        "expected a redirect, got: {:?}",
        text
    );
    assert!(text.contains("https://example.org/"));

    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server must exit")
        .unwrap()
        .unwrap();
}

/// The same client handshake against the correct secret must be *accepted* —
/// the bridge then tries the real upstream, which we cannot do offline, but
/// the important part (no rejection, session counted) happens before that.
#[tokio::test]
async fn correct_secret_handshake_is_accepted() {
    let port = free_port();
    let stats = Arc::new(Stats::new());
    let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(1);

    let server = tokio::spawn(run_server_with_stats(
        test_config(port),
        stats.clone(),
        shutdown_rx,
    ));
    wait_listening(port).await;
    // The readiness probe counts as a client of its own, and its session fails
    // the moment the probe drops. Let that settle before taking the baseline,
    // otherwise the deltas below race with the probe's own failure.
    tokio::time::sleep(Duration::from_millis(120)).await;
    while stats.snapshot(0.0, 0.0).connections_active > 0 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let baseline = stats.snapshot(0.0, 0.0);

    let secret = hex::decode("00112233445566778899aabbccddeeff").unwrap();
    let handshake = build_client_handshake(&secret, &PROTO_TAG_ABRIDGED, 2);

    let mut client = connect(port).await;
    client.write_all(&handshake).await.unwrap();

    // Give the server a moment to parse the handshake, then drop the client:
    // the upstream attempt is irrelevant here and would hit the network.
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(client);

    let after = stats.snapshot(0.0, 0.0);
    assert_eq!(after.connections_total - baseline.connections_total, 1);
    assert_eq!(
        after.connections_bad - baseline.connections_bad,
        0,
        "a correct handshake must never count as bad"
    );
    assert!(
        after.last_error.is_empty() || after.last_error == baseline.last_error,
        "no new error should be recorded, got: {:?}",
        after.last_error
    );

    shutdown_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("server must exit")
        .unwrap()
        .unwrap();
}
