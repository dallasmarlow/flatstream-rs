//! Defines the `Checksum` trait and concrete implementations.

use crate::error::{Error, Result};

/// A trait for checksum algorithms.
///
/// The on-wire width is the associated `SIZE` constant, so the width dispatch
/// in the framing layer constant-folds by construction and a nonstandard
/// width can never silently widen to 8 bytes. Serialization is centralized in
/// [`write_bytes`](Self::write_bytes)/[`read_bytes`](Self::read_bytes) —
/// pure over byte slices, little-endian, byte-exact for any `SIZE ≤ 8`
/// (I/O stays in the deframer, where the error handling lives).
pub trait Checksum {
    /// Width in bytes of the checksum field on the wire. Must be ≤ 8: values
    /// travel as the low `SIZE` little-endian bytes of the `u64` this trait
    /// works in (enforced at compile time where framers are constructed).
    const SIZE: usize;

    /// Calculates the checksum for the given payload.
    fn calculate(&self, payload: &[u8]) -> u64;

    /// Verifies the checksum. Returns `Ok(())` if it matches.
    ///
    /// The comparison is modulo the wire width: only the low `SIZE` bytes
    /// travel, so a `calculate` wider than `SIZE` (legal for custom
    /// implementations) must not fail verification against its own truncated
    /// wire form. For the built-ins the mask is the identity and folds away.
    fn verify(&self, expected: u64, payload: &[u8]) -> Result<()> {
        let calculated = self.calculate(payload);
        let mask = width_mask(Self::SIZE);
        if calculated & mask == expected & mask {
            Ok(())
        } else {
            Err(Error::checksum_mismatch(expected, calculated))
        }
    }

    /// Serializes `value` into its exact on-wire form: the low `SIZE` bytes,
    /// little-endian, staged in `out`. Returns the wire bytes.
    #[inline]
    fn write_bytes<'a>(&self, value: u64, out: &'a mut [u8; 8]) -> &'a [u8] {
        out.copy_from_slice(&value.to_le_bytes());
        &out[..Self::SIZE]
    }

    /// Parses the `SIZE`-byte little-endian wire field back into a `u64`.
    /// `bytes` must hold at least `SIZE` bytes.
    #[inline]
    fn read_bytes(&self, bytes: &[u8]) -> u64 {
        let mut buf = [0u8; 8];
        buf[..Self::SIZE].copy_from_slice(&bytes[..Self::SIZE]);
        u64::from_le_bytes(buf)
    }
}

/// The value mask a `size`-byte wire field can carry: low `8 * size` bits.
const fn width_mask(size: usize) -> u64 {
    if size >= 8 {
        u64::MAX
    } else {
        (1u64 << (8 * size)) - 1
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
    /// XXH3 produces a 64-bit (8-byte) hash.
    const SIZE: usize = 8;

    fn calculate(&self, payload: &[u8]) -> u64 {
        xxhash_rust::xxh3::xxh3_64(payload)
    }
}

/// Provides an implementation of the CRC-32 (ISO-HDLC/IEEE, the zlib
/// polynomial) checksum algorithm, as computed by `crc32fast`.
///
/// NOT CRC-32C (Castagnoli). An earlier doc comment misnamed this; the
/// implementation — and therefore the wire format — has always been
/// ISO-HDLC (`"123456789"` → `0xCBF43926`), and the known-answer test below
/// pins it. If Castagnoli (hardware `crc32c` instructions) is ever wanted,
/// it must be a new algorithm, not a silent swap: the bytes differ.
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
    /// CRC32 produces a 32-bit (4-byte) hash.
    const SIZE: usize = 4;

    fn calculate(&self, payload: &[u8]) -> u64 {
        // crc32fast returns a u32, so we cast it to u64 for trait compatibility.
        crc32fast::hash(payload) as u64
    }
}

/// Provides an implementation of the CRC-16/XMODEM checksum algorithm
/// (polynomial 0x1021, init 0x0000; `"123456789"` → `0x31C3`, pinned by the
/// known-answer test). Note: this is the XMODEM variant, not CRC-16/CCITT-FALSE
/// (init 0xFFFF, KAT 0x29B1) — the two share a polynomial and are often
/// conflated. Ideal for extremely small packets where every byte counts.
#[cfg(feature = "crc16")]
#[derive(Default, Clone, Copy)]
pub struct Crc16;

#[cfg(feature = "crc16")]
impl Crc16 {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(feature = "crc16")]
impl Checksum for Crc16 {
    /// CRC16 produces a 16-bit (2-byte) hash.
    const SIZE: usize = 2;

    fn calculate(&self, payload: &[u8]) -> u64 {
        // crc16 returns a u16, so we cast it to u64 for trait compatibility.
        crc16::State::<crc16::XMODEM>::calculate(payload) as u64
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
    /// No checksum bytes are written.
    const SIZE: usize = 0;

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

    #[cfg(feature = "crc32")]
    #[test]
    fn test_crc32_checksum() {
        let checksum = Crc32::new();
        let payload = b"test data";
        let result = checksum.calculate(payload);
        assert_ne!(result, 0);
        assert!(checksum.verify(result, payload).is_ok());
        assert!(checksum.verify(result + 1, payload).is_err());
        assert_eq!(Crc32::SIZE, 4);
    }

    #[cfg(feature = "crc16")]
    #[test]
    fn test_crc16_checksum() {
        let checksum = Crc16::new();
        let payload = b"test data";
        let result = checksum.calculate(payload);
        assert_ne!(result, 0);
        assert!(checksum.verify(result, payload).is_ok());
        assert!(checksum.verify(result + 1, payload).is_err());
        assert_eq!(Crc16::SIZE, 2);
    }

    #[test]
    fn test_checksum_sizes() {
        assert_eq!(NoChecksum::SIZE, 0);

        #[cfg(feature = "xxhash")]
        assert_eq!(XxHash64::SIZE, 8);

        #[cfg(feature = "crc32")]
        assert_eq!(Crc32::SIZE, 4);

        #[cfg(feature = "crc16")]
        assert_eq!(Crc16::SIZE, 2);
    }

    #[test]
    fn known_answer_vectors() {
        // Standard KAT input "123456789" — pins each algorithm's identity so a
        // dependency swap or misnamed algorithm can never change the wire
        // bytes silently. These values are normative for WIRE_FORMAT_SPEC.md §5.
        #[cfg(feature = "crc32")]
        assert_eq!(
            Crc32::new().calculate(b"123456789"),
            0xCBF4_3926,
            "CRC-32/ISO-HDLC (NOT Castagnoli, which is 0xE3069283)"
        );
        #[cfg(feature = "crc16")]
        assert_eq!(
            Crc16::new().calculate(b"123456789"),
            0x31C3,
            "CRC-16/XMODEM"
        );
        #[cfg(feature = "xxhash")]
        assert_eq!(
            XxHash64::new().calculate(b"123456789"),
            0x72DC_B18B_67A1_7DFF,
            "XXH3-64"
        );
    }

    #[test]
    fn test_write_read_bytes_roundtrip_and_width() {
        // The default helpers must be byte-exact for every width: exactly
        // SIZE little-endian bytes on the wire, value recovered modulo the
        // width's truncation.
        fn roundtrip<C: Checksum>(alg: &C, value: u64) {
            let mut out = [0u8; 8];
            let wire = alg.write_bytes(value, &mut out);
            assert_eq!(wire.len(), C::SIZE);
            assert_eq!(wire, &value.to_le_bytes()[..C::SIZE]);
            let mask = if C::SIZE == 8 {
                u64::MAX
            } else {
                (1u64 << (8 * C::SIZE as u32)) - 1
            };
            assert_eq!(alg.read_bytes(wire), value & mask);
        }

        let probe = 0x1122_3344_5566_7788u64;
        roundtrip(&NoChecksum::new(), probe);
        #[cfg(feature = "xxhash")]
        roundtrip(&XxHash64::new(), probe);
        #[cfg(feature = "crc32")]
        roundtrip(&Crc32::new(), probe);
        #[cfg(feature = "crc16")]
        roundtrip(&Crc16::new(), probe);
    }
}
