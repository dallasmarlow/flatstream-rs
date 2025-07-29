//! Defines the `Checksum` trait and concrete implementations.

use crate::error::{Error, Result};

/// A trait for checksum algorithms.
pub trait Checksum {
    /// Calculates the checksum for the given payload.
    fn calculate(&self, payload: &[u8]) -> u64;

    /// Verifies the checksum. Returns `Ok(())` if it matches.
    fn verify(&self, expected: u64, payload: &[u8]) -> Result<()> {
        let calculated = self.calculate(payload);
        if calculated == expected {
            Ok(())
        } else {
            Err(Error::checksum_mismatch(expected, calculated))
        }
    }
}

/// Provides an implementation of the XXH3 64-bit hash algorithm.
#[cfg(feature = "xxhash")]
#[derive(Default, Clone, Copy)]
pub struct XxHash64;

#[cfg(feature = "xxhash")]
impl XxHash64 {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(feature = "xxhash")]
impl Checksum for XxHash64 {
    fn calculate(&self, payload: &[u8]) -> u64 {
        xxhash_rust::xxh3::xxh3_64(payload)
    }
}

/// Provides an implementation of the CRC32c (Castagnoli) checksum algorithm.
#[cfg(feature = "crc32")]
#[derive(Default, Clone, Copy)]
pub struct Crc32;

#[cfg(feature = "crc32")]
impl Crc32 {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(feature = "crc32")]
impl Checksum for Crc32 {
    fn calculate(&self, payload: &[u8]) -> u64 {
        // crc32fast returns a u32, so we cast it to u64 for trait compatibility.
        crc32fast::hash(payload) as u64
    }
}

// For backward compatibility, we can provide a "None" checksum implementation
/// A no-op checksum implementation for when checksums are not needed.
#[derive(Default, Clone, Copy)]
pub struct NoChecksum;

impl NoChecksum {
    pub fn new() -> Self {
        Self
    }
}

impl Checksum for NoChecksum {
    fn calculate(&self, _payload: &[u8]) -> u64 {
        0
    }

    fn verify(&self, _expected: u64, _payload: &[u8]) -> Result<()> {
        // Always succeeds - no verification needed
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "xxhash")]
    #[test]
    fn test_xxhash64_checksum() {
        let checksum = XxHash64::new();
        let payload = b"test data";
        let result = checksum.calculate(payload);
        assert_ne!(result, 0);
        assert!(checksum.verify(result, payload).is_ok());
        assert!(checksum.verify(result + 1, payload).is_err());
    }

    #[test]
    fn test_no_checksum() {
        let checksum = NoChecksum::new();
        let payload = b"test data";
        let result = checksum.calculate(payload);
        assert_eq!(result, 0);
        assert!(checksum.verify(0, payload).is_ok());
        assert!(checksum.verify(123, payload).is_ok()); // Always succeeds
    }

    #[cfg(feature = "xxhash")]
    #[test]
    fn test_checksum_consistency() {
        let checksum = XxHash64::new();
        let payload = b"consistent test data";
        let result1 = checksum.calculate(payload);
        let result2 = checksum.calculate(payload);
        assert_eq!(result1, result2);
    }
}
