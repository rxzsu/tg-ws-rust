//! Socket creation helpers with I/O buffer tuning.
//!
//! All hot-path sockets (client listener, upstream WS/TCP) are created here
//! so `SO_RCVBUF`/`SO_SNDBUF` match the user-configured `buffer_size`
//! instead of OS defaults.

use std::io;
use tokio::net::{TcpListener, TcpStream};

/// Clamp the user-configured I/O buffer into a sane range (4 KB – 1 MB).
pub fn clamp_io_buf(n: usize) -> usize {
    n.clamp(4096, 1024 * 1024)
}

fn apply_buf(sock: &socket2::SockRef<'_>, io_buf: usize) {
    let buf = clamp_io_buf(io_buf);
    if let Err(e) = sock.set_recv_buffer_size(buf) {
        tracing::debug!("set_recv_buffer_size({}) failed: {:?}", buf, e);
    }
    if let Err(e) = sock.set_send_buffer_size(buf) {
        tracing::debug!("set_send_buffer_size({}) failed: {:?}", buf, e);
    }
}

async fn resolve(host: &str, port: u16) -> io::Result<std::net::SocketAddr> {
    resolve_host(host, port).await
}

/// Resolve `host:port` to a socket address (first result).
pub async fn resolve_host(host: &str, port: u16) -> io::Result<std::net::SocketAddr> {
    tokio::net::lookup_host((host, port))
        .await?
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "DNS resolution returned no addresses"))
}

/// Bind a listener with `SO_REUSEADDR` set (quick restart after config apply).
pub async fn bind_listener(host: &str, port: u16) -> io::Result<TcpListener> {
    let addr = resolve(host, port).await?;
    let domain = if addr.is_ipv4() {
        socket2::Domain::IPV4
    } else {
        socket2::Domain::IPV6
    };
    let sock = socket2::Socket::new(domain, socket2::Type::STREAM, Some(socket2::Protocol::TCP))?;
    // SO_REUSEADDR means two different things per platform. On Unix it only
    // skips TIME_WAIT, which is what we want for instant restarts. On Windows
    // it lets a *second* socket bind an address that is already listening —
    // two proxy instances then split incoming connections unpredictably, and
    // the "port busy" diagnostic never fires. Windows keeps its default
    // exclusive behaviour instead.
    #[cfg(not(windows))]
    sock.set_reuse_address(true)?;
    sock.bind(&addr.into())?;
    sock.listen(1024)?;
    sock.set_nonblocking(true)?;
    TcpListener::from_std(sock.into())
}

/// Connect a TCP stream with `TCP_NODELAY` and tuned kernel buffers.
///
/// Tokio's connect tries every resolved address until one succeeds, so a
/// host that answers only on IPv6 (or only on IPv4) is still reachable —
/// the previous first-address-only resolve picked one family and dropped
/// the other.
///
/// NOTE: the connect itself must go through Tokio (`TcpStream::connect`),
/// which drives the non-blocking handshake to completion. A raw blocking
/// `connect()` on a non-blocking socket fails with `WOULDBLOCK` (WSA 10035
/// on Windows). Buffer sizes are applied right after, which is equivalent
/// for all subsequent transfers.
pub async fn tcp_connect(host: &str, port: u16, io_buf: usize) -> io::Result<TcpStream> {
    let stream = TcpStream::connect((host, port)).await?;
    tune_accepted(stream, io_buf)
}

/// Apply kernel buffer sizes to an already-accepted client stream.
pub fn tune_accepted(stream: TcpStream, io_buf: usize) -> io::Result<TcpStream> {
    let std_stream = stream.into_std()?;
    let sock = socket2::SockRef::from(&std_stream);
    let _ = sock.set_tcp_nodelay(true);
    apply_buf(&sock, io_buf);
    sock.set_nonblocking(true)?;
    TcpStream::from_std(std_stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test: `tcp_connect` must complete the handshake instead
    /// of failing with `WOULDBLOCK` (WSA 10035 on Windows).
    #[tokio::test]
    async fn tcp_connect_loopback_ok() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (client, server) = tokio::join!(
            tcp_connect("127.0.0.1", port, 65536),
            listener.accept()
        );
        assert!(client.is_ok(), "client connect failed: {:?}", client.err());
        assert!(server.is_ok());
    }
}
