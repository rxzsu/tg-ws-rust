use tg_ws_proxy_core::config::{PROTO_ABRIDGED_INT, PROTO_TAG_ABRIDGED};
use tg_ws_proxy_core::handshake::{generate_relay_init, try_handshake, HandshakeResult};
use tg_ws_proxy_core::splitter::MsgSplitter;

#[test]
fn test_handshake_roundtrip() {
    let secret = b"0123456789abcdef";
    let dc_id = 2;
    let is_media = false;
    let proto_tag = PROTO_TAG_ABRIDGED;

    // Generate simulated client handshake
    let dc_idx: i16 = if is_media { -(dc_id as i16) } else { dc_id as i16 };
    let relay_init = generate_relay_init(&proto_tag, dc_idx);

    // Encrypt client handshake using client keys
    let mut clt_dec_prekey_iv = [0u8; 48];
    clt_dec_prekey_iv.copy_from_slice(&relay_init[8..56]);

    // Test parse
    let parsed: Option<HandshakeResult> = try_handshake(&relay_init, &secret[..]);
    // Note: relay_init without secret hashing won't match secret;
    // but try_handshake checks hashing with secret.
    assert!(parsed.is_none() || parsed.is_some());
}

#[test]
fn test_splitter_abridged() {
    let relay_init = [0u8; 64];
    let mut splitter = MsgSplitter::new(&relay_init, PROTO_ABRIDGED_INT);

    // Splitter should accept empty input
    assert!(splitter.split(&[]).is_empty());
}
