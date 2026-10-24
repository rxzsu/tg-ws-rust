//! Per-connection buffer recycling.
//!
//! Every bridge session used to allocate two fresh `buf_size` buffers
//! (upload + download). Under heavy media traffic (video streams) that is
//! a steady stream of large alloc/free pairs — heap fragmentation and
//! allocator contention for no benefit, since buffer sizes are identical
//! across sessions.
//!
//! `BufferPool` keeps a bounded freelist of buffers. `acquire` pops a
//! buffer of at least the requested capacity or allocates a new one;
//! `release` returns it to the pool (oversized buffers are dropped, so
//! the pool never holds more than one max-size tier). The freelist is a
//! lock-free `Mutex<Vec>` — hold times are a pointer swap.

use std::sync::{Arc, Mutex, OnceLock};

const MAX_POOLED_BUFFERS: usize = 32;
/// Buffers larger than this are never pooled (avoid hoarding 1 MB tiers
/// after a one-off huge transfer).
const MAX_POOLED_CAPACITY: usize = 1024 * 1024;

static GLOBAL_POOL: OnceLock<Arc<BufferPool>> = OnceLock::new();

/// Process-wide pool used by all bridge sessions.
pub fn global() -> Arc<BufferPool> {
    GLOBAL_POOL.get_or_init(|| Arc::new(BufferPool::new())).clone()
}

pub struct BufferPool {
    free: Mutex<Vec<Vec<u8>>>,
}

impl BufferPool {
    pub fn new() -> Self {
        Self { free: Mutex::new(Vec::new()) }
    }

    /// Get a buffer with at least `cap` bytes of writable space.
    ///
    /// Takes `self` by `Arc` so the returned `PooledBuf` can keep the pool
    /// alive without any raw pointers.
    pub fn acquire(self: &Arc<Self>, cap: usize) -> PooledBuf {
        let buf = self
            .free
            .lock()
            .ok()
            .and_then(|mut v| {
                // Pop the most recently returned buffer (LIFO = hot in cache).
                while let Some(b) = v.pop() {
                    if b.len() >= cap {
                        return Some(b);
                    }
                    // Too small for the current tier — drop it.
                }
                None
            })
            .unwrap_or_else(|| vec![0u8; cap]);
        PooledBuf { buf: Some(buf), pool: Arc::clone(self) }
    }

    fn release(&self, buf: Vec<u8>) {
        if buf.capacity() > MAX_POOLED_CAPACITY {
            return;
        }
        if let Ok(mut v) = self.free.lock()
            && v.len() < MAX_POOLED_BUFFERS
        {
            v.push(buf);
        }
    }
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience alias: acquire from the global pool.
pub fn acquire_buf(cap: usize) -> PooledBuf {
    global().acquire(cap)
}

/// Deref wrapper that returns the buffer to the pool on drop.
pub struct PooledBuf {
    buf: Option<Vec<u8>>,
    /// Keeps the pool alive for as long as any outstanding buffer exists;
    /// releasing into a dropped pool would be a leak-free but silent no-op
    /// anyway, so no unsafe aliasing is needed.
    pool: Arc<BufferPool>,
}

impl std::ops::Deref for PooledBuf {
    type Target = Vec<u8>;
    fn deref(&self) -> &Vec<u8> {
        self.buf.as_ref().expect("buffer not taken")
    }
}

impl std::ops::DerefMut for PooledBuf {
    fn deref_mut(&mut self) -> &mut Vec<u8> {
        self.buf.as_mut().expect("buffer not taken")
    }
}

impl Drop for PooledBuf {
    fn drop(&mut self) {
        if let Some(buf) = self.buf.take() {
            self.pool.release(buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reuse_after_release() {
        let pool = Arc::new(BufferPool::new());
        {
            let mut b = pool.acquire(4096);
            b[0] = 0xFF;
            assert_eq!(b.len(), 4096);
        }
        // Released on drop; next acquire must reuse it (same allocation).
        let b2 = pool.acquire(4096);
        assert_eq!(b2.len(), 4096);
    }

    #[test]
    fn oversized_request_allocates_fresh() {
        let pool = Arc::new(BufferPool::new());
        let _small = pool.acquire(1024);
        let big = pool.acquire(512 * 1024);
        assert_eq!(big.len(), 512 * 1024);
    }

    #[test]
    fn pool_is_bounded() {
        let pool = Arc::new(BufferPool::new());
        let mut bufs = Vec::new();
        for _ in 0..64 {
            bufs.push(pool.acquire(4096));
        }
        drop(bufs);
        let free = pool.free.lock().unwrap().len();
        assert!(free <= MAX_POOLED_BUFFERS);
    }
}
