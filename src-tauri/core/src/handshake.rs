use crate::config::{
    PROTO_ABRIDGED_INT, PROTO_INTERMEDIATE_INT, PROTO_PADDED_INTERMEDIATE_INT,
    PROTO_TAG_ABRIDGED, PROTO_TAG_INTERMEDIATE, PROTO_TAG_SECURE,
};
use aes::cipher::{KeyIvInit, StreamCipher};
use aes::Aes256;
use ctr::Ctr128BE;
use rand::RngExt;
use sha2::{Digest, Sha256};

type Aes256Ctr = Ctr128BE<Aes256>;

pub const HANDSHAKE_LEN: usize = 64;
pub const SKIP_LEN: usize = 8;
pub const PREKEY_LEN: usize = 32;
pub const IV_LEN: usize = 16;
pub const PROTO_TAG_POS: usize = 56;
pub const DC_IDX_POS: usize = 60;

#[derive(Debug, Clone)]
pub struct HandshakeResult {
    pub dc_id: i32,
    pub is_media: bool,
    pub proto_tag: [u8; 4],
    pub proto_int: u32,
    pub dec_prekey_and_iv: [u8; 48],
}

pub fn try_handshake(handshake: &[u8; 64], secret: &[u8]) -> Option<HandshakeResult> {
    let mut dec_prekey_and_iv = [0u8; 48];
    dec_prekey_and_iv.copy_from_slice(&handshake[SKIP_LEN..SKIP_LEN + PREKEY_LEN + IV_LEN]);

    let mut hasher = Sha256::new();
    hasher.update(&dec_prekey_and_iv[0..32]);
    hasher.update(secret);
    let dec_key: [u8; 32] = hasher.finalize().into();

    let mut dec_iv = [0u8; 16];
    dec_iv.copy_from_slice(&dec_prekey_and_iv[32..48]);

    let mut decryptor = Aes256Ctr::new((&dec_key).into(), (&dec_iv).into());
    let mut decrypted = *handshake;
    decryptor.apply_keystream(&mut decrypted);

    let mut proto_tag = [0u8; 4];
    proto_tag.copy_from_slice(&decrypted[PROTO_TAG_POS..PROTO_TAG_POS + 4]);

    let proto_int = if proto_tag == PROTO_TAG_ABRIDGED {
        PROTO_ABRIDGED_INT
    } else if proto_tag == PROTO_TAG_INTERMEDIATE {
        PROTO_INTERMEDIATE_INT
    } else if proto_tag == PROTO_TAG_SECURE {
        PROTO_PADDED_INTERMEDIATE_INT
    } else {
        return None;
    };

    let dc_idx = i16::from_le_bytes([decrypted[DC_IDX_POS], decrypted[DC_IDX_POS + 1]]) as i32;
    let dc_id = dc_idx.abs();
    let is_media = dc_idx < 0;

    Some(HandshakeResult {
        dc_id,
        is_media,
        proto_tag,
        proto_int,
        dec_prekey_and_iv,
    })
}

pub fn generate_relay_init(proto_tag: &[u8; 4], dc_idx: i16) -> [u8; 64] {
    let mut rng = rand::rng();
    let mut rnd = [0u8; HANDSHAKE_LEN];

    loop {
        rng.fill(&mut rnd);
        if rnd[0] == 0xef {
            continue;
        }
        let start = &rnd[0..4];
        if start == b"HEAD"
            || start == b"POST"
            || start == b"GET "
            || start == b"\xee\xee\xee\xee"
            || start == b"\xdd\xdd\xdd\xdd"
            || start == b"\x16\x03\x01\x02"
        {
            continue;
        }
        if &rnd[4..8] == b"\x00\x00\x00\x00" {
            continue;
        }
        break;
    }

    let mut enc_key = [0u8; 32];
    enc_key.copy_from_slice(&rnd[SKIP_LEN..SKIP_LEN + PREKEY_LEN]);
    let mut enc_iv = [0u8; 16];
    enc_iv.copy_from_slice(&rnd[SKIP_LEN + PREKEY_LEN..SKIP_LEN + PREKEY_LEN + IV_LEN]);

    let mut encryptor = Aes256Ctr::new((&enc_key).into(), (&enc_iv).into());

    let mut extra = [0u8; 2];
    rng.fill(&mut extra);

    let dc_bytes = dc_idx.to_le_bytes();
    let mut tail_plain = [0u8; 8];
    tail_plain[0..4].copy_from_slice(proto_tag);
    tail_plain[4..6].copy_from_slice(&dc_bytes);
    tail_plain[6..8].copy_from_slice(&extra);

    let mut encrypted_full = rnd;
    encryptor.apply_keystream(&mut encrypted_full);

    let mut keystream_tail = [0u8; 8];
    for i in 0..8 {
        keystream_tail[i] = encrypted_full[56 + i] ^ rnd[56 + i];
    }

    let mut encrypted_tail = [0u8; 8];
    for i in 0..8 {
        encrypted_tail[i] = tail_plain[i] ^ keystream_tail[i];
    }

    let mut result = rnd;
    result[PROTO_TAG_POS..HANDSHAKE_LEN].copy_from_slice(&encrypted_tail);
    result
}
