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
    /// Start of unconsumed data. Consumed prefix is compacted lazily
    /// instead of `drain`ing on every call.
    head: usize,
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
            head: 0,
            disabled: false,
        }
    }

    /// Split an incoming ciphertext chunk into packet slices.
    ///
    /// Returns slices borrowing the internal buffer — no per-packet
    /// allocation (previously every packet was copied into its own `Vec`).
    /// The slices stay valid until the next `split`/`flush` call.
    pub fn split(&mut self, chunk: &[u8]) -> Vec<&[u8]> {
        if chunk.is_empty() {
            return Vec::new();
        }
        // Previous call's slices are dead by contract: reclaim its prefix first.
        self.compact_if_needed();
        if self.disabled {
            self.cipher_buf.extend_from_slice(chunk);
            // Same single-slice shape as below; caller sends it as one frame.
            return vec![&self.cipher_buf[..]];
        }

        self.cipher_buf.extend_from_slice(chunk);
        // Decrypt straight into the tail of the parse buffer: no temp copy.
        let plain_start = self.plain_buf.len();
        self.plain_buf.extend_from_slice(chunk);
        self.dec.apply_keystream(&mut self.plain_buf[plain_start..]);

        let mut ranges = Vec::new();
        let mut offset = self.head;
        let buf_len = self.cipher_buf.len();

        while offset < buf_len {
            let avail = buf_len - offset;
            match self.next_packet_len(offset, avail) {
                Some(len) if len > 0 => {
                    ranges.push((offset, offset + len));
                    offset += len;
                }
                Some(_) => {
                    // <= 0: disable parsing and emit remainder
                    ranges.push((offset, buf_len));
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

        self.head = offset;

        let buf = &self.cipher_buf;
        ranges.into_iter().map(|(s, e)| &buf[s..e]).collect()
    }

    /// Drop the consumed prefix. Runs at the start of the next call, when
    /// previously returned slices are dead by contract. Fully-consumed
    /// buffers reset without memmove; partial prefixes compact lazily.
    fn compact_if_needed(&mut self) {
        if self.head == 0 {
            return;
        }
        if self.head >= self.cipher_buf.len() {
            self.cipher_buf.clear();
            self.plain_buf.clear();
        } else if self.head >= 1 << 20 {
            self.cipher_buf.drain(0..self.head);
            self.plain_buf.drain(0..self.head);
        } else {
            return;
        }
        self.head = 0;
    }

    pub fn flush(&mut self) -> Option<Vec<u8>> {
        if self.head >= self.cipher_buf.len() {
            self.cipher_buf.clear();
            self.plain_buf.clear();
            self.head = 0;
            None
        } else {
            let tail = self.cipher_buf[self.head..].to_vec();
            self.cipher_buf.clear();
            self.plain_buf.clear();
            self.head = 0;
            Some(tail)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PROTO_ABRIDGED_INT;

    /// Rebuild the splitter's decryption keystream (mirrors `MsgSplitter::new`)
    /// to craft valid ciphertext test vectors.
    fn test_encryptor(relay_init: &[u8; 64]) -> Aes256Ctr {
        let mut key = [0u8; 32];
        key.copy_from_slice(&relay_init[8..40]);
        let mut iv = [0u8; 16];
        iv.copy_from_slice(&relay_init[40..56]);
        let mut enc = Aes256Ctr::new((&key).into(), (&iv).into());
        let mut zero64 = [0u8; 64];
        enc.apply_keystream(&mut zero64);
        enc
    }

    fn encrypt(enc: &mut Aes256Ctr, plain: &[u8]) -> Vec<u8> {
        let mut ct = plain.to_vec();
        enc.apply_keystream(&mut ct);
        ct
    }

    #[test]
    fn split_complete_packets_roundtrip() {
        let relay_init = [0x42u8; 64];
        let mut enc = test_encryptor(&relay_init);
        // Two abridged packets: [len=1 word][4B] + [len=2 words][8B].
        let plain: Vec<u8> = vec![
            0x01, 1, 2, 3, 4,
            0x02, 5, 6, 7, 8, 9, 10, 11, 12,
        ];
        let ct = encrypt(&mut enc, &plain);

        let mut sp = MsgSplitter::new(&relay_init, PROTO_ABRIDGED_INT);
        let parts = sp.split(&ct);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], &ct[0..5]);
        assert_eq!(parts[1], &ct[5..14]);
        // Nothing left: second call yields nothing.
        assert!(sp.split(&[]).is_empty());
        assert!(sp.flush().is_none());
    }

    #[test]
    fn split_partial_waits_for_rest() {
        let relay_init = [0x77u8; 64];
        let mut enc = test_encryptor(&relay_init);
        let plain: Vec<u8> = vec![0x01, 9, 9, 9, 9];
        let ct = encrypt(&mut enc, &plain);

        let mut sp = MsgSplitter::new(&relay_init, PROTO_ABRIDGED_INT);
        // Only 3 of 5 bytes: incomplete, no output.
        assert!(sp.split(&ct[..3]).is_empty());
        // Remaining bytes complete the packet.
        let parts = sp.split(&ct[3..]);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0], &ct[..]);
    }

    #[test]
    fn flush_returns_unfinished_tail() {
        let relay_init = [0x99u8; 64];
        let mut enc = test_encryptor(&relay_init);
        let plain: Vec<u8> = vec![0x01, 7, 7, 7, 7];
        let ct = encrypt(&mut enc, &plain);

        let mut sp = MsgSplitter::new(&relay_init, PROTO_ABRIDGED_INT);
        assert!(sp.split(&ct[..2]).is_empty());
        let tail = sp.flush().expect("tail must be returned");
        assert_eq!(tail, ct[..2]);
    }
}
