use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

/// SHA-256 digest of an MCP bearer secret. Raw bearer values are never stored here.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SecretHash([u8; 32]);

impl SecretHash {
    /// Hashes a raw bearer secret at the trust boundary.
    #[must_use]
    pub fn from_bearer(secret: &str) -> Self {
        Self(Sha256::digest(secret.as_bytes()).into())
    }

    /// Reconstructs a digest loaded from protected persistence.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SecretError> {
        let digest: [u8; 32] = bytes
            .try_into()
            .map_err(|_| SecretError::InvalidDigestLength(bytes.len()))?;
        Ok(Self(digest))
    }

    /// Returns digest bytes for persistence. These bytes are not the bearer secret.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Compares digests without data-dependent early exit.
    #[must_use]
    pub fn constant_time_eq(&self, candidate: &Self) -> bool {
        bool::from(self.0.ct_eq(&candidate.0))
    }
}

impl std::fmt::Debug for SecretHash {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretHash([REDACTED])")
    }
}

/// Generated bearer secret returned only by create/rotate operations.
pub struct GeneratedSecret {
    raw: String,
    last4: String,
}

impl GeneratedSecret {
    /// Constructs a one-time result from a freshly generated secret.
    #[must_use]
    pub fn new(raw: String) -> Self {
        let last4 = raw
            .chars()
            .rev()
            .take(4)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        Self { raw, last4 }
    }

    /// Non-secret suffix suitable for UI identification.
    #[must_use]
    pub fn last4(&self) -> &str {
        &self.last4
    }

    /// Consumes the wrapper and reveals the raw bearer exactly once.
    #[must_use]
    pub fn expose_once(self) -> String {
        self.raw
    }
}

impl std::fmt::Debug for GeneratedSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GeneratedSecret")
            .field("raw", &"[REDACTED]")
            .field("last4", &self.last4)
            .finish()
    }
}

/// Secret validation errors.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SecretError {
    /// Persisted digest has an unexpected byte length.
    #[error("secret digest must contain 32 bytes, got {0}")]
    InvalidDigestLength(usize),
}
