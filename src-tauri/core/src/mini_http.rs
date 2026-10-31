//! Minimal HTTP/1.1 response reading for the hand-rolled TLS clients
//! (GitHub API, DoH resolvers). Only what those call sites need: a bounded
//! read of one response, a status check, and Content-Length / chunked body
//! decoding. The old call sites `read_to_end`-ed the raw stream and parsed
//! the body past the `\r\n\r\n` split, so a 403 rate-limit page silently
//! looked like "no update available" and a `Transfer-Encoding: chunked`
//! response fed hex chunk sizes into the body.

use tokio::io::AsyncReadExt;

pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Read one full HTTP/1.1 response off the stream. Callers issue
/// `Connection: close` requests, so the body ends at EOF. The whole response
/// is capped at `cap` bytes: a hostile or misbehaving endpoint cannot balloon
/// memory regardless of what headers claim.
pub async fn read_response<S>(stream: &mut S, cap: usize) -> Result<HttpResponse, String>
where
    S: AsyncReadExt + Unpin,
{
    let mut raw = Vec::with_capacity(8192);
    let mut chunk = [0u8; 8192];
    loop {
        if raw.len() > cap {
            return Err(format!("HTTP response exceeds {cap} bytes"));
        }
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|e| format!("HTTP read failed: {e}"))?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&chunk[..n]);
    }
    parse_response(&raw, cap)
}

/// Pure parser over a fully-read response. `cap` bounds the decoded body.
pub fn parse_response(raw: &[u8], cap: usize) -> Result<HttpResponse, String> {
    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("invalid HTTP response: no header terminator")?;
    let head = std::str::from_utf8(&raw[..header_end])
        .map_err(|_| "invalid HTTP response: non-UTF8 headers")?;
    let mut lines = head.split("\r\n");

    let status_line = lines.next().ok_or("empty HTTP response")?;
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("malformed HTTP status line: {status_line}"))?;

    let mut chunked = false;
    let mut content_length: Option<usize> = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        match name.as_str() {
            "transfer-encoding" => {
                if value.to_ascii_lowercase().contains("chunked") {
                    chunked = true;
                }
            }
            "content-length" => {
                content_length = value.parse().ok();
            }
            _ => {}
        }
    }

    let body_bytes = &raw[header_end + 4..];
    let body = if chunked {
        decode_chunked(body_bytes, cap)?
    } else if let Some(len) = content_length {
        if len > cap {
            return Err(format!("HTTP body of {len} bytes exceeds {cap} byte cap"));
        }
        // Short bodies (truncated read) are an error: the caller relies on
        // the declared length for framed responses.
        if body_bytes.len() < len {
            return Err(format!(
                "truncated HTTP body: declared {len} bytes, got {}",
                body_bytes.len()
            ));
        }
        body_bytes[..len].to_vec()
    } else {
        if body_bytes.len() > cap {
            return Err(format!("HTTP body exceeds {cap} byte cap"));
        }
        body_bytes.to_vec()
    };

    Ok(HttpResponse { status, body })
}

/// Decode a chunked transfer-encoded body (hex size per line, `0` ends the
/// stream). Trailers after the last chunk are skipped.
fn decode_chunked(body: &[u8], cap: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut rest = body;
    loop {
        let line_end = rest
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or("truncated chunked body: no chunk size line")?;
        let size_str = std::str::from_utf8(&rest[..line_end])
            .map_err(|_| "invalid chunk size line")?;
        let size_str = size_str.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_str, 16)
            .map_err(|_| format!("invalid chunk size: {size_str}"))?;
        rest = &rest[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if out.len() + size > cap {
            return Err(format!("decoded chunked body exceeds {cap} byte cap"));
        }
        if rest.len() < size + 2 {
            return Err("truncated chunked body: chunk spans past EOF".into());
        }
        out.extend_from_slice(&rest[..size]);
        rest = &rest[size..];
        // Consume the CRLF after the chunk data; tolerate a bare EOF if the
        // server closed right after the final data byte.
        if rest.starts_with(b"\r\n") {
            rest = &rest[2..];
        } else if !rest.is_empty() {
            return Err("chunk data not followed by CRLF".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_length_body() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\nhello";
        let resp = parse_response(raw, 1024).unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"hello");
    }

    #[test]
    fn parses_chunked_body() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                    5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let resp = parse_response(raw, 1024).unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body, b"hello world");
    }

    #[test]
    fn chunked_body_is_capped() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
                    FFFFFFFFFF\r\naaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n0\r\n\r\n";
        assert!(parse_response(raw, 64).is_err(), "a huge chunk must be rejected");
    }

    #[test]
    fn status_line_is_parsed_and_errors_surface() {
        let raw = b"HTTP/1.1 403 Forbidden\r\nContent-Length: 2\r\n\r\n{}";
        let resp = parse_response(raw, 1024).unwrap();
        assert_eq!(resp.status, 403);
        assert_eq!(resp.body, b"{}");

        assert!(parse_response(b"garbage", 1024).is_err());
        assert!(parse_response(b"HTTP/1.1 ???\r\n\r\n", 1024).is_err());
    }

    #[test]
    fn content_length_declared_larger_than_cap_is_rejected() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\ntiny";
        assert!(parse_response(raw, 1024).is_err());
    }

    #[test]
    fn read_response_caps_raw_stream() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // A stream that never stops producing: the cap, not EOF, ends the read.
            struct Endless;
            impl tokio::io::AsyncRead for Endless {
                fn poll_read(
                    self: std::pin::Pin<&mut Self>,
                    _cx: &mut std::task::Context<'_>,
                    buf: &mut tokio::io::ReadBuf<'_>,
                ) -> std::task::Poll<std::io::Result<()>> {
                    let filler = [b'a'; 8192];
                    let n = buf.remaining().min(filler.len());
                    buf.put_slice(&filler[..n]);
                    std::task::Poll::Ready(Ok(()))
                }
            }
            let mut stream = Endless;
            assert!(read_response(&mut stream, 16 * 1024).await.is_err());
        });
    }
}
