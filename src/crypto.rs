//! Secret-key management and small primitives.
//!
//! All secrets are encrypted at rest with a 256-bit key derived from a 32-byte
//! random secret persisted in `<data_dir>/.q-key` (mode 0600). Session and
//! invite tokens are stored only as SHA-256 hashes so a database leak does not
//! reveal live tokens.

use std::path::Path;

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use anyhow::Context;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::RngCore;
use sha2::{Digest, Sha256};

const NONCE_LEN: usize = 12;

/// A 256-bit key used to encrypt worker secrets at rest.
pub struct SecretKey([u8; 32]);

impl SecretKey {
    pub fn load_or_create(data_dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir)
            .with_context(|| format!("creating data dir {}", data_dir.display()))?;
        let path = data_dir.join(".q-key");
        if path.exists() {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("reading secret key {}", path.display()))?;
            if bytes.len() != 32 {
                anyhow::bail!("{} has invalid length (expected 32 bytes)", path.display());
            }
            let mut k = [0u8; 32];
            k.copy_from_slice(&bytes);
            return Ok(SecretKey(k));
        }
        let mut k = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut k);
        std::fs::write(&path, &k)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(SecretKey(k))
    }

    /// Encrypt a secret string (worker passwords). Returns `base64url(nonce || ciphertext)`.
    pub fn encrypt(&self, plain: &str) -> anyhow::Result<String> {
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.0));
        let mut nonce = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce);
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), plain.as_bytes())
            .map_err(|e| anyhow::anyhow!("encryption failed: {e}"))?;
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ciphertext);
        Ok(URL_SAFE_NO_PAD.encode(out))
    }

    /// Decrypt a value produced by [`SecretKey::encrypt`].
    pub fn decrypt(&self, data: &str) -> anyhow::Result<String> {
        let raw = URL_SAFE_NO_PAD
            .decode(data)
            .context("invalid ciphertext encoding")?;
        if raw.len() < NONCE_LEN {
            anyhow::bail!("invalid ciphertext length");
        }
        let (nonce, ct) = raw.split_at(NONCE_LEN);
        let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.0));
        let pt = cipher
            .decrypt(Nonce::from_slice(nonce), ct)
            .map_err(|e| anyhow::anyhow!("decryption failed: {e}"))?;
        String::from_utf8(pt).context("decrypted data is not UTF-8")
    }
}

/// A URL-safe random token (32 bytes). Used for sessions and invite links.
pub fn new_token() -> String {
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    URL_SAFE_NO_PAD.encode(b)
}

/// SHA-256 hex digest. Used to store one-way hashes of session/invite tokens.
pub fn hash_secret(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    let out = h.finalize();
    out.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::SecretKey;

    #[test]
    fn encrypt_roundtrip() {
        let key = SecretKey([7u8; 32]);
        let ct = key.encrypt("p@ssw0rd").unwrap();
        assert_ne!(ct, "p@ssw0rd");
        assert_eq!(key.decrypt(&ct).unwrap(), "p@ssw0rd");
    }

    #[test]
    fn decrypt_with_wrong_key_fails() {
        let a = SecretKey([1u8; 32]);
        let b = SecretKey([2u8; 32]);
        let ct = a.encrypt("secret").unwrap();
        assert!(b.decrypt(&ct).is_err());
    }
}
