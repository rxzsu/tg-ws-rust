use aes::cipher::{KeyIvInit, StreamCipher};
use aes::Aes256;
use ctr::Ctr128BE;
use sha2::{Digest, Sha256};

pub type Aes256Ctr = Ctr128BE<Aes256>;

pub struct CryptoCtx {
    pub clt_dec: Aes256Ctr,
    pub clt_enc: Aes256Ctr,
    pub tg_enc: Aes256Ctr,
    pub tg_dec: Aes256Ctr,
}

impl CryptoCtx {
    pub fn new(client_dec_prekey_iv: &[u8; 48], secret: &[u8], relay_init: &[u8; 64]) -> Self {
        // clt_dec
        let mut hasher = Sha256::new();
        hasher.update(&client_dec_prekey_iv[0..32]);
        hasher.update(secret);
        let clt_dec_key: [u8; 32] = hasher.finalize().into();
        let mut clt_dec_iv = [0u8; 16];
        clt_dec_iv.copy_from_slice(&client_dec_prekey_iv[32..48]);

        // clt_enc: reversed prekey_iv
        let mut reversed_prekey_iv = *client_dec_prekey_iv;
        reversed_prekey_iv.reverse();

        let mut hasher = Sha256::new();
        hasher.update(&reversed_prekey_iv[0..32]);
        hasher.update(secret);
        let clt_enc_key: [u8; 32] = hasher.finalize().into();
        let mut clt_enc_iv = [0u8; 16];
        clt_enc_iv.copy_from_slice(&reversed_prekey_iv[32..48]);

        let mut clt_dec = Aes256Ctr::new((&clt_dec_key).into(), (&clt_dec_iv).into());
        let clt_enc = Aes256Ctr::new((&clt_enc_key).into(), (&clt_enc_iv).into());

        // Fast-forward clt_dec by 64 bytes
        let mut zero64 = [0u8; 64];
        clt_dec.apply_keystream(&mut zero64);

        // Telegram relay ciphers: standard MTProto obfuscation
        let mut relay_enc_key = [0u8; 32];
        relay_enc_key.copy_from_slice(&relay_init[8..40]);
        let mut relay_enc_iv = [0u8; 16];
        relay_enc_iv.copy_from_slice(&relay_init[40..56]);

        let mut relay_dec_prekey_iv = [0u8; 48];
        relay_dec_prekey_iv.copy_from_slice(&relay_init[8..56]);
        relay_dec_prekey_iv.reverse();

        let mut relay_dec_key = [0u8; 32];
        relay_dec_key.copy_from_slice(&relay_dec_prekey_iv[0..32]);
        let mut relay_dec_iv = [0u8; 16];
        relay_dec_iv.copy_from_slice(&relay_dec_prekey_iv[32..48]);

        let mut tg_enc = Aes256Ctr::new((&relay_enc_key).into(), (&relay_enc_iv).into());
        let tg_dec = Aes256Ctr::new((&relay_dec_key).into(), (&relay_dec_iv).into());

        // Fast-forward tg_enc by 64 bytes
        let mut zero64 = [0u8; 64];
        tg_enc.apply_keystream(&mut zero64);

        Self {
            clt_dec,
            clt_enc,
            tg_enc,
            tg_dec,
        }
    }
}
