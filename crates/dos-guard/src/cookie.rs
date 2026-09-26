//! Stateless handshake cookies.
//!
//! First contact gets a cookie instead of a KEM response:
//! `blake3_keyed(secret, addr ‖ epoch)[..16]`. The peer echoes it in its next
//! hello; the node recomputes it (one hash, no state) and only then does KEM
//! work. A spoofed source address never sees its cookie, so it cannot make
//! the node encapsulate. Epochs rotate every [`EPOCH_MS`]; the previous
//! epoch's cookie is still accepted so a peer mid-handshake at the boundary
//! is not dropped.

use crate::Addr;

/// Cookie validity window.
pub const EPOCH_MS: u64 = 30_000;
/// Cookie length on the wire.
pub const COOKIE_LEN: usize = 16;

/// Issues and checks cookies under one secret.
pub struct CookieJar {
    secret: [u8; 32],
}

impl CookieJar {
    /// A jar keyed by `secret`, which the node draws at start-up and never shares.
    #[must_use]
    pub fn new(secret: [u8; 32]) -> Self {
        Self { secret }
    }

    fn at(&self, addr: Addr, epoch: u64) -> [u8; COOKIE_LEN] {
        let mut h = blake3::Hasher::new_keyed(&self.secret);
        h.update(&addr.bytes());
        h.update(&epoch.to_le_bytes());
        let mut out = [0u8; COOKIE_LEN];
        out.copy_from_slice(&h.finalize().as_bytes()[..COOKIE_LEN]);
        out
    }

    /// The cookie for `addr` now.
    #[must_use]
    pub fn issue(&self, addr: Addr, now_ms: u64) -> [u8; COOKIE_LEN] {
        self.at(addr, now_ms / EPOCH_MS)
    }

    /// Whether `cookie` was issued to `addr` in this epoch or the last.
    #[must_use]
    pub fn check(&self, addr: Addr, cookie: &[u8; COOKIE_LEN], now_ms: u64) -> bool {
        let epoch = now_ms / EPOCH_MS;
        // Constant-time enough for a 128-bit MAC compared once: a timing leak
        // reveals at most a prefix of a value the attacker must guess whole.
        self.at(addr, epoch) == *cookie || (epoch > 0 && self.at(addr, epoch - 1) == *cookie)
    }
}
