//! Buffers that hold a secret: locked in RAM so they never reach swap, and
//! zeroed when dropped. A header value made from one shares its memory, so
//! the zeroing waits for the last handle, the one this type keeps.

use bytes::{Bytes, BytesMut};
use zeroize::Zeroize;

/// The memory could not be locked; the request that needed it is refused.
#[derive(Debug, thiserror::Error)]
#[error("secret memory could not be locked: {0}")]
pub struct LockError(String);

/// Secret bytes, locked in RAM.
pub struct Locked {
    bytes: Bytes,
    _lock: Option<region::LockGuard>,
}

impl std::fmt::Debug for Locked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Locked(***)")
    }
}

impl Locked {
    /// A locked copy of `content`.
    pub fn new(content: &[u8]) -> Result<Self, LockError> {
        let mut buf = BytesMut::with_capacity(content.len());
        buf.extend_from_slice(content);
        let lock = if buf.is_empty() {
            None
        } else {
            Some(region::lock(buf.as_ptr(), buf.len()).map_err(|e| LockError(e.to_string()))?)
        };
        Ok(Self { bytes: buf.freeze(), _lock: lock })
    }

    /// The bytes, sharing this buffer's memory.
    pub fn bytes(&self) -> Bytes {
        self.bytes.clone()
    }

    /// The bytes, to read.
    pub fn expose(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for Locked {
    fn drop(&mut self) {
        // Zeroable only once every other handle (a header value, a request
        // the client keeps) is gone, which the proxy ensures by dropping them
        // first.
        if let Ok(mut owned) = std::mem::take(&mut self.bytes).try_into_mut() {
            owned.as_mut().zeroize();
        }
    }
}
