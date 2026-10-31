use crate::config::{
    PROTO_ABRIDGED_INT, PROTO_INTERMEDIATE_INT, PROTO_PADDED_INTERMEDIATE_INT,
};
use aes::cipher::{KeyIvInit, StreamCipher};
use aes::Aes256;
use ctr::Ctr128BE;

type Aes256Ctr = Ctr128BE<Aes256>;

/// Hard cap on a single claimed packet length. A real MTProto message never
/// approaches this, so a larger declared length is malformed input: parsing
/// disables itself (passthrough) instead of buffering gigabytes in flight —
/// any client with the secret could otherwise request a ~2 GB packet in
/// intermediate mode (or ~64 MB in abridged) and balloon both buffers.
const MAX_SPLIT_PACKET_LEN: usize = 16 * 1024 * 1024;

pub struct MsgSplitter {
    dec: Aes256Ctr,
    proto: u32,
    cipher_buf: Vec<u8>,
    plain_buf: Vec<u8>,
    /// Start of unconsumed data. Consumed prefix is compacted lazily
    /// instead of `drain`ing on every call.
    head: usize,
    disabled: bool,
    /// Whether the first packet has already been emitted. Only the first
    /// (most fingerprintable) packet of the session gets randomized
    /// sub-fragmentation.
    first_packet_done: bool,
}

/// Randomize the fragmentation of the first packet: emit its first chunk
/// as 1..=7 separate sub-frames with random boundaries inside the first
/// `win` bytes. Returns boundaries relative to `packet`.
fn randomized_fragment_boundaries(packet: &[u8]) -> Vec<usize> {
    use rand::RngExt;
    // Nothing to cut: a single-byte packet stays intact.
    if packet.len() < 2 {
        return Vec::new();
    }
    let mut rng = rand::rng();
    let win = packet.len().min(96);
    let n_cuts = rng.random_range(1..=3);
    let mut cuts: Vec<usize> = Vec::with_capacity(n_cuts as usize);
    for _ in 0..n_cuts {
        let c = rng.random_range(1..win);
        if !cuts.contains(&c) {
            cuts.push(c);
        }
    }
    cuts.sort_unstable();
    cuts
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
            first_packet_done: false,
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
            // Parsing is off: everything buffered before this call was already
            // emitted and consumed. Clearing here keeps only the current chunk
            // in the buffer, so every chunk is returned exactly once (the old
            // code relied on `compact_if_needed`, which fires exactly once
            // after the disable and then never again — `head` stays 0 — so
            // each subsequent call re-returned the whole accumulated buffer).
            self.cipher_buf.clear();
            self.plain_buf.clear();
            self.head = 0;
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
        let mut slices: Vec<&[u8]> = ranges.iter().map(|(s, e)| &buf[*s..*e]).collect();

        // Adaptive DPI evasion: the very first packet of the session gets
        // randomized sub-fragmentation. Everything after stays protocol-
        // correct (frames are still exact packet boundaries — Telegram's
        // WS endpoint accepts both fragmented and single-frame writes).
        if !self.first_packet_done && !slices.is_empty() {
            let first = slices[0];
            let bounds = randomized_fragment_boundaries(first);
            let start = ranges[0].0;
            let mut frag: Vec<&[u8]> = Vec::with_capacity(bounds.len() + 1);
            let mut prev = 0usize;
            for b in &bounds {
                frag.push(&buf[start + prev..start + b]);
                prev = *b;
            }
            frag.push(&buf[start + prev..start + first.len()]);
            // Replace the first slice with the fragmented pieces: keeping the
            // whole packet in the list as well would send those bytes twice.
            let mut rest = slices.split_off(1);
            let mut out = frag;
            out.append(&mut rest);
            slices = out;
            self.first_packet_done = true;
        }

        slices
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
        if packet_len > MAX_SPLIT_PACKET_LEN {
            // Malformed claim (abridged caps at ~64 MB): disable parsing
            // (passthrough) instead of buffering the shortfall forever.
            return Some(0);
        }
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
        if packet_len > MAX_SPLIT_PACKET_LEN {
            // Malformed claim (intermediate allows ~2 GB): disable parsing
            // instead of buffering gigabytes in cipher/plain buffers.
            return Some(0);
        }
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
    use crate::config::{PROTO_ABRIDGED_INT, PROTO_INTERMEDIATE_INT};

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

    /// Flatten emitted frames back into the original byte stream.
    fn join(parts: &[&[u8]]) -> Vec<u8> {
        parts.iter().flat_map(|p| p.iter().copied()).collect()
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
        assert!(parts.len() >= 2, "expected at least two frames");
        // The first packet is deliberately sub-fragmented, but concatenating
        // every frame must reproduce the ciphertext byte for byte.
        let joined: Vec<u8> = join(&parts);
        assert_eq!(joined, ct);
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
        assert_eq!(join(&parts), ct);
    }

    #[test]
    fn first_packet_fragments_do_not_duplicate_bytes() {
        let relay_init = [0x11u8; 64];
        let mut enc = test_encryptor(&relay_init);
        let plain: Vec<u8> = vec![0x02, 1, 2, 3, 4, 5, 6, 7, 8];
        let ct = encrypt(&mut enc, &plain);

        let mut sp = MsgSplitter::new(&relay_init, PROTO_ABRIDGED_INT);
        let parts = sp.split(&ct);
        let total: usize = parts.iter().map(|p| p.len()).sum();
        assert_eq!(total, ct.len(), "fragmentation must not duplicate the first packet");
        assert_eq!(join(&parts), ct);
    }

    #[test]
    fn later_packets_are_not_fragmented() {
        let relay_init = [0x22u8; 64];
        let mut enc = test_encryptor(&relay_init);
        let first: Vec<u8> = vec![0x01, 1, 2, 3, 4];
        let second: Vec<u8> = vec![0x01, 5, 6, 7, 8];
        let ct_first = encrypt(&mut enc, &first);
        let ct_second = encrypt(&mut enc, &second);

        let mut sp = MsgSplitter::new(&relay_init, PROTO_ABRIDGED_INT);
        assert_eq!(join(&sp.split(&ct_first)), ct_first);
        // Second packet is emitted as a single frame (no extra splitting).
        let parts = sp.split(&ct_second);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0], &ct_second[..]);
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

    /// Regression: after the splitter disables itself, every later chunk must
    /// come through exactly once. The old disabled branch kept the previous
    /// chunk in the buffer (compaction fires only on the first post-disable
    /// call, then `head` stays 0), so each new call re-returned everything
    /// accumulated before it — quadratic duplicate traffic.
    #[test]
    fn disabled_mode_returns_each_chunk_exactly_once() {
        let relay_init = [0x5Au8; 64];
        let mut enc = test_encryptor(&relay_init);
        // A zero-length abridged packet (first byte 0x00) is malformed and
        // switches the splitter to passthrough.
        let ct_bad = encrypt(&mut enc, &[0x00, 0x11, 0x22, 0x33]);
        let mut sp = MsgSplitter::new(&relay_init, PROTO_ABRIDGED_INT);
        let first = sp.split(&ct_bad);
        assert_eq!(join(&first), ct_bad, "the malformed chunk itself is emitted once");

        let ct2 = encrypt(&mut enc, &[0x44, 0x55, 0x66]);
        assert_eq!(join(&sp.split(&ct2)), ct2);
        // The old bug shows on this third chunk: chunk2 was re-sent with it.
        let ct3 = encrypt(&mut enc, &[0x77, 0x88]);
        assert_eq!(join(&sp.split(&ct3)), ct3);
    }

    /// Regression: a packet-length claim beyond the cap must disable parsing
    /// instead of buffering the shortfall (remote-memory-exhaustion vector).
    #[test]
    fn oversized_packet_claim_disables_parsing() {
        let relay_init = [0x66u8; 64];
        let mut enc = test_encryptor(&relay_init);
        let mut sp = MsgSplitter::new(&relay_init, PROTO_INTERMEDIATE_INT);
        // Intermediate header claiming just over the cap (~2 GB allowed raw).
        let mut plain = ((16 * 1024 * 1024 + 1) as u32).to_le_bytes().to_vec();
        plain.extend_from_slice(&[0xAA; 32]);
        let ct = encrypt(&mut enc, &plain);
        // Passed through as a single frame instead of waiting for ~2 GB.
        assert_eq!(join(&sp.split(&ct)), ct);
    }
}
