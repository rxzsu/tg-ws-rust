use crate::config::{
    PROTO_ABRIDGED_INT, PROTO_INTERMEDIATE_INT, PROTO_PADDED_INTERMEDIATE_INT,
};
use aes::cipher::{KeyIvInit, StreamCipher};
use aes::Aes256;
use ctr::Ctr128BE;

type Aes256Ctr = Ctr128BE<Aes256>;

pub struct MsgSplitter {
    dec: Aes256Ctr,
    proto: u32,
    cipher_buf: Vec<u8>,
    plain_buf: Vec<u8>,
    disabled: bool,
}

impl MsgSplitter {
    pub fn new(relay_init: &[u8; 64], proto: u32) -> Self {
        let mut key = [0u8; 32];
        key.copy_from_slice(&relay_init[8..40]);
        let mut iv = [0u8; 16];
        iv.copy_from_slice(&relay_init[40..56]);

        let mut dec = Aes256Ctr::new((&key).into(), (&iv).into());
        let mut zero64 = [0u8; 64];
        dec.apply_keystream(&mut zero64);

        Self {
            dec,
            proto,
            cipher_buf: Vec::with_capacity(8192),
            plain_buf: Vec::with_capacity(8192),
            disabled: false,
        }
    }

    pub fn split(&mut self, chunk: &[u8]) -> Vec<Vec<u8>> {
        if chunk.is_empty() {
            return Vec::new();
        }
        if self.disabled {
            return vec![chunk.to_vec()];
        }

        self.cipher_buf.extend_from_slice(chunk);
        let mut decrypted_chunk = chunk.to_vec();
        self.dec.apply_keystream(&mut decrypted_chunk);
        self.plain_buf.extend_from_slice(&decrypted_chunk);

        let mut parts = Vec::new();
        let mut offset = 0;
        let buf_len = self.cipher_buf.len();

        while offset < buf_len {
            let avail = buf_len - offset;
            match self.next_packet_len(offset, avail) {
                Some(len) if len > 0 => {
                    parts.push(self.cipher_buf[offset..offset + len].to_vec());
                    offset += len;
                }
                Some(_) => {
                    // <= 0: disable parsing and emit remainder
                    parts.push(self.cipher_buf[offset..].to_vec());
                    offset = buf_len;
                    self.disabled = true;
                    break;
                }
                None => {
                    // Need more bytes
                    break;
                }
            }
        }

        if offset > 0 {
            self.cipher_buf.drain(0..offset);
            self.plain_buf.drain(0..offset);
        }

        parts
    }

    pub fn flush(&mut self) -> Vec<Vec<u8>> {
        if self.cipher_buf.is_empty() {
            Vec::new()
        } else {
            let tail = std::mem::take(&mut self.cipher_buf);
            self.plain_buf.clear();
            vec![tail]
        }
    }

    fn next_packet_len(&self, offset: usize, avail: usize) -> Option<usize> {
        if avail == 0 {
            return None;
        }
        if self.proto == PROTO_ABRIDGED_INT {
            self.next_abridged_len(offset, avail)
        } else if self.proto == PROTO_INTERMEDIATE_INT
            || self.proto == PROTO_PADDED_INTERMEDIATE_INT
        {
            self.next_intermediate_len(offset, avail)
        } else {
            Some(0)
        }
    }

    fn next_abridged_len(&self, offset: usize, avail: usize) -> Option<usize> {
        let first = self.plain_buf[offset];
        let (header_len, payload_len) = if first == 0x7F || first == 0xFF {
            if avail < 4 {
                return None;
            }
            let bytes = [
                self.plain_buf[offset + 1],
                self.plain_buf[offset + 2],
                self.plain_buf[offset + 3],
                0,
            ];
            let len_words = u32::from_le_bytes(bytes) as usize;
            (4, len_words * 4)
        } else {
            let len_words = (first & 0x7F) as usize;
            (1, len_words * 4)
        };

        if payload_len == 0 {
            return Some(0);
        }
        let packet_len = header_len + payload_len;
        if avail < packet_len {
            None
        } else {
            Some(packet_len)
        }
    }

    fn next_intermediate_len(&self, offset: usize, avail: usize) -> Option<usize> {
        if avail < 4 {
            return None;
        }
        let bytes = [
            self.plain_buf[offset],
            self.plain_buf[offset + 1],
            self.plain_buf[offset + 2],
            self.plain_buf[offset + 3],
        ];
        let raw = u32::from_le_bytes(bytes) & 0x7FFFFFFF;
        let payload_len = raw as usize;
        if payload_len == 0 {
            return Some(0);
        }
        let packet_len = 4 + payload_len;
        if avail < packet_len {
            None
        } else {
            Some(packet_len)
        }
    }
}
