use crate::error::{Error, Result};

/// Supported checksum types for data integrity verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksumType {
    /// No checksum verification (maximum performance).
    None,
    /// XXH3 64-bit hash for fast checksumming.
    XxHash64,
}

impl ChecksumType {
    /// Calculate the checksum for the given payload.
    ///
    /// # Arguments
    /// * `payload` - The data to calculate checksum for
    ///
    /// # Returns
    /// The calculated checksum value, or 0 if checksumming is disabled.
    pub fn calculate_checksum(&self, payload: &[u8]) -> u64 {
        match self {
            ChecksumType::None => 0,
            ChecksumType::XxHash64 => xxhash_rust::xxh3::xxh3_64(payload),
        }
    }

    /// Verify that the expected checksum matches the calculated checksum for the payload.
    ///
    /// # Arguments
    /// * `expected_checksum` - The expected checksum value
    /// * `payload` - The data to verify
    ///
    /// # Returns
    /// `Ok(())` if checksums match, `Err(ChecksumMismatch)` otherwise.
    pub fn verify_checksum(&self, expected_checksum: u64, payload: &[u8]) -> Result<()> {
        match self {
            ChecksumType::None => {
                // When checksums are disabled, we don't verify anything
                Ok(())
            }
            ChecksumType::XxHash64 => {
                let calculated = xxhash_rust::xxh3::xxh3_64(payload);
                if calculated == expected_checksum {
                    Ok(())
                } else {
                    Err(Error::checksum_mismatch(expected_checksum, calculated))
                }
            }
        }
    }

    /// Get the size in bytes of the checksum field for this checksum type.
    pub fn size(&self) -> usize {
        match self {
            ChecksumType::None => 0,
            ChecksumType::XxHash64 => 8, // u64 = 8 bytes
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_none_checksum() {
        let checksum_type = ChecksumType::None;
        let payload = b"test data";

        let checksum = checksum_type.calculate_checksum(payload);
        assert_eq!(checksum, 0);

        // Should always pass verification when disabled
        assert!(checksum_type.verify_checksum(0, payload).is_ok());
        assert!(checksum_type.verify_checksum(123, payload).is_ok());

        assert_eq!(checksum_type.size(), 0);
    }

    #[test]
    fn test_xxhash64_checksum() {
        let checksum_type = ChecksumType::XxHash64;
        let payload = b"test data";

        let checksum = checksum_type.calculate_checksum(payload);
        assert_ne!(checksum, 0); // XXH3 should produce a non-zero hash

        // Should pass verification with correct checksum
        assert!(checksum_type.verify_checksum(checksum, payload).is_ok());

        // Should fail verification with incorrect checksum
        assert!(checksum_type
            .verify_checksum(checksum + 1, payload)
            .is_err());

        assert_eq!(checksum_type.size(), 8);
    }

    #[test]
    fn test_checksum_consistency() {
        let checksum_type = ChecksumType::XxHash64;
        let payload = b"consistent test data";

        let checksum1 = checksum_type.calculate_checksum(payload);
        let checksum2 = checksum_type.calculate_checksum(payload);

        // Same input should produce same checksum
        assert_eq!(checksum1, checksum2);
    }

    #[test]
    fn test_different_payloads_different_checksums() {
        let checksum_type = ChecksumType::XxHash64;
        let payload1 = b"test data 1";
        let payload2 = b"test data 2";

        let checksum1 = checksum_type.calculate_checksum(payload1);
        let checksum2 = checksum_type.calculate_checksum(payload2);

        // Different inputs should produce different checksums
        assert_ne!(checksum1, checksum2);
    }
}
