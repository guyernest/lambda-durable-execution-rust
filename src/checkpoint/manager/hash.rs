use super::*;

impl CheckpointManager {
    /// Hash an operation ID for storage.
    ///
    /// We use SHA-256 truncated to 128 bits (32 hex chars). The JS SDK uses MD5-16 for
    /// speed and the Python SDK uses BLAKE2b-64; SHA-256 is widely understood and avoids
    /// MD5 while keeping IDs reasonably short.
    pub fn hash_id(id: &str) -> String {
        use sha2::{Digest, Sha256};
        use std::fmt::Write as _;

        let digest = Sha256::digest(id.as_bytes());
        let mut hex = String::with_capacity(32);
        for byte in digest.iter().take(16) {
            let _ = write!(hex, "{byte:02x}");
        }
        hex
    }
}
