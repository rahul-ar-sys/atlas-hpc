//! Software TEE (Trusted Execution Environment) guard.
//!
//! In production, this would call into Intel TDX or AMD SEV-SNP attestation
//! APIs to seal data in hardware enclaves.  In the MVP it provides an
//! AES-256-GCM software guard with the same interface.

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use serde::{de::DeserializeOwned, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// TeeGuard
// ─────────────────────────────────────────────────────────────────────────────

/// Wraps sensitive financial data (weights, graph edges) in a TEE boundary.
///
/// On hardware TEE platforms (Intel TDX, AMD SEV-SNP), `seal` would invoke
/// `tee_seal()` syscall / vmcall and `unseal` would call `tee_unseal()`.
/// In the MVP software simulation, AES-256-GCM is used.
pub struct SoftwareTeeGuard {
    cipher: Aes256Gcm,
}

impl SoftwareTeeGuard {
    /// Create a guard with a freshly generated ephemeral 256-bit key.
    ///
    /// In production, the key would come from the hardware TEE key derivation
    /// function seeded by platform measurements (PCR values / SEV attestation).
    pub fn new_ephemeral() -> Self {
        let key = Aes256Gcm::generate_key(OsRng);
        Self {
            cipher: Aes256Gcm::new(&key),
        }
    }

    /// Create a guard with a caller-supplied 32-byte key (for deterministic tests).
    pub fn with_key(key_bytes: &[u8; 32]) -> Self {
        let key = Key::<Aes256Gcm>::from_slice(key_bytes);
        Self {
            cipher: Aes256Gcm::new(key),
        }
    }

    /// Seal (encrypt + authenticate) a serialisable value.
    ///
    /// Returns `(nonce_bytes, ciphertext)`.  Both must be stored and provided
    /// to [`unseal`] for decryption.
    pub fn seal<T: Serialize>(&self, value: &T) -> Result<(Vec<u8>, Vec<u8>), TeeError> {
        let plaintext = serde_json::to_vec(value).map_err(|e| TeeError::Serialization(e.to_string()))?;
        let nonce = Aes256Gcm::generate_nonce(OsRng);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_slice())
            .map_err(|e| TeeError::Encryption(e.to_string()))?;
        Ok((nonce.to_vec(), ciphertext))
    }

    /// Unseal (decrypt + verify) a previously sealed value.
    pub fn unseal<T: DeserializeOwned>(&self, nonce_bytes: &[u8], ciphertext: &[u8]) -> Result<T, TeeError> {
        let nonce = Nonce::from_slice(nonce_bytes);
        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| TeeError::Decryption(e.to_string()))?;
        serde_json::from_slice(&plaintext).map_err(|e| TeeError::Serialization(e.to_string()))
    }
}

/// Errors produced by the TEE guard.
#[derive(Debug)]
pub enum TeeError {
    /// Serialisation failure.
    Serialization(String),
    /// AES-GCM encryption failure.
    Encryption(String),
    /// AES-GCM decryption / authentication-tag failure.
    Decryption(String),
}

impl std::fmt::Display for TeeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TeeError::Serialization(e) => write!(f, "TeeError::Serialization: {e}"),
            TeeError::Encryption(e) => write!(f, "TeeError::Encryption: {e}"),
            TeeError::Decryption(e) => write!(f, "TeeError::Decryption: {e}"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct SensitivePayload {
        weight: f32,
        entity_id: u64,
        secret: String,
    }

    #[test]
    fn seal_unseal_round_trips() {
        let key = [0xABu8; 32];
        let guard = SoftwareTeeGuard::with_key(&key);

        let payload = SensitivePayload {
            weight: 0.87,
            entity_id: 12345,
            secret: "XAUUSD-proprietary-weight".into(),
        };

        let (nonce, ct) = guard.seal(&payload).expect("seal failed");
        let recovered: SensitivePayload = guard.unseal(&nonce, &ct).expect("unseal failed");

        assert_eq!(recovered, payload);
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let key = [0xCDu8; 32];
        let guard = SoftwareTeeGuard::with_key(&key);
        let (nonce, mut ct) = guard.seal(&42u64).expect("seal failed");
        ct[0] ^= 0xFF; // flip bits → authentication tag mismatch
        assert!(guard.unseal::<u64>(&nonce, &ct).is_err());
    }

    #[test]
    fn ephemeral_guard_produces_different_key_each_time() {
        let g1 = SoftwareTeeGuard::new_ephemeral();
        let g2 = SoftwareTeeGuard::new_ephemeral();
        let payload = "atlas-test-payload";
        let (nonce1, ct1) = g1.seal(&payload).unwrap();
        // g2 uses a different key — should fail to decrypt g1's ciphertext.
        let result: Result<String, _> = g2.unseal(&nonce1, &ct1);
        assert!(result.is_err(), "cross-key decryption must fail");
    }
}
