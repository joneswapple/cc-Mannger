//! AES-256-GCM sealing for small values (OAuth tokens) at rest.

use std::io::Write;
use std::path::Path;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use rand::RngCore;

/// Key file created under the data dir when no key is configured. Losing it
/// makes every stored account token unreadable.
pub const KEY_FILE: &str = "secret.key";

const NONCE_LEN: usize = 12;

pub struct SecretBox {
    cipher: Aes256Gcm,
}

impl SecretBox {
    pub fn new(key: &[u8]) -> Result<Self> {
        if key.len() != 32 {
            bail!("secret key must be 32 bytes, got {}", key.len());
        }
        Ok(Self {
            cipher: Aes256Gcm::new_from_slice(key).map_err(|e| anyhow!("{e}"))?,
        })
    }

    /// Returns nonce || ciphertext. `aad` binds the ciphertext to its context
    /// (which account and field) so rows can't be swapped.
    pub fn seal(&self, plain: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ct = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad })
            .map_err(|_| anyhow!("secret: encrypt failed"))?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ct);
        Ok(out)
    }

    pub fn open(&self, sealed: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
        if sealed.len() < NONCE_LEN {
            bail!("secret: ciphertext too short");
        }
        let (nonce, ct) = sealed.split_at(NONCE_LEN);
        self.cipher
            .decrypt(Nonce::from_slice(nonce), Payload { msg: ct, aad })
            .map_err(|_| anyhow!("secret: decrypt failed (wrong key?)"))
    }

    /// A key given as base64 text, e.g. the contents of a `secret.key` file.
    pub fn from_base64(s: &str) -> Result<Self> {
        Self::new(&decode_key(s)?)
    }

    /// Resolves the key: a configured base64 value wins; otherwise
    /// `data_dir/secret.key` is read, or created with a random key (0600).
    pub fn load(configured: &str, data_dir: &Path) -> Result<Self> {
        if !configured.is_empty() {
            return Self::new(&decode_key(configured).context("secret_key")?);
        }
        let path = data_dir.join(KEY_FILE);
        match std::fs::read_to_string(&path) {
            Ok(s) => {
                return Self::new(&decode_key(&s).with_context(|| path.display().to_string())?)
            }
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(e).with_context(|| format!("read {}", path.display()))
            }
            Err(_) => {}
        }
        let mut key = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut key);
        std::fs::create_dir_all(data_dir)?;
        let mut f = create_private(&path).with_context(|| format!("create {}", path.display()))?;
        writeln!(f, "{}", B64.encode(key))?;
        f.sync_all()?;
        Self::new(&key)
    }
}

fn decode_key(s: &str) -> Result<Vec<u8>> {
    let key = B64
        .decode(s.trim())
        .context("expected a base64-encoded 32-byte key")?;
    if key.len() != 32 {
        bail!("expected a 32-byte key, got {} bytes", key.len());
    }
    Ok(key)
}

#[cfg(unix)]
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_round_trip_and_aad_binding() {
        let b = SecretBox::new(&[7u8; 32]).unwrap();
        let sealed = b.seal(b"tok", b"account:1:access").unwrap();
        assert!(!sealed.windows(3).any(|w| w == b"tok"));
        assert_eq!(b.open(&sealed, b"account:1:access").unwrap(), b"tok");
        assert!(b.open(&sealed, b"account:2:access").is_err());
    }

    #[test]
    fn load_creates_then_reuses_key_file() {
        let dir = tempfile::tempdir().unwrap();
        let b1 = SecretBox::load("", dir.path()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.path().join(KEY_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let sealed = b1.seal(b"x", b"").unwrap();
        let b2 = SecretBox::load("", dir.path()).unwrap();
        assert_eq!(b2.open(&sealed, b"").unwrap(), b"x");
    }

    #[test]
    fn load_configured_key() {
        let dir = tempfile::tempdir().unwrap();
        assert!(SecretBox::load(&B64.encode([1u8; 32]), dir.path()).is_ok());
        assert!(SecretBox::load("c2hvcnQ=", dir.path()).is_err());
    }
}
