//! Credentials CC Manager issues itself: terminal API keys (`sk-cp-…`),
//! single-use invite tokens, and the admin token. Only hashes are stored.

mod admin;
mod invites;
mod keys;

pub use admin::{Admin, AdminUnauthorized};
pub use invites::{InviteError, InviteInfo, Invites, Redeemed};
pub use keys::{Identity, KeyError, KeyInfo, Keys, UserKeys, KEY_PREFIX};

use rand::RngCore;

/// bcrypt cost; matches Go's `bcrypt.DefaultCost` so hashes migrated from
/// the Go cliproxy verify the same way.
pub(crate) const BCRYPT_COST: u32 = 10;

pub(crate) fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut b);
    hex::encode(b)
}

pub(crate) fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(s.as_bytes()))
}
