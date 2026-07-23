use flatstream::framing::DefaultFramer;
use flatstream::*;

#[test]
fn defaultframer_layout() {
    // Purpose: Verify DefaultFramer writes [4-byte LE length | payload].
    let payload = b"abc";
    let mut out = Vec::new();
    DefaultFramer.frame_and_write(&mut out, payload).unwrap();
    assert_eq!(out.len(), 4 + payload.len());
    let len = u32::from_le_bytes([out[0], out[1], out[2], out[3]]) as usize;
    assert_eq!(len, payload.len());
    assert_eq!(&out[4..], payload);
}

#[cfg(feature = "xxhash")]
#[test]
fn checksumframer_layout_xxhash64() {
    // Purpose: Verify ChecksumFramer::<XxHash64> writes [4-byte LE length | 8-byte checksum | payload].
    use flatstream::framing::ChecksumFramer;
    let payload = b"abc";
    let mut out = Vec::new();
    let framer = ChecksumFramer::new(XxHash64::new());
    framer.frame_and_write(&mut out, payload).unwrap();
    assert_eq!(out.len(), 4 + 8 + payload.len());
    let len = u32::from_le_bytes([out[0], out[1], out[2], out[3]]) as usize;
    assert_eq!(len, payload.len());
    let _cksum = u64::from_le_bytes(out[4..12].try_into().unwrap());
    assert_eq!(&out[12..], payload);
}

#[cfg(feature = "crc32")]
#[test]
fn checksumframer_layout_crc32() {
    // Purpose: Verify ChecksumFramer::<Crc32> writes [4-byte LE length | 4-byte checksum | payload].
    use flatstream::framing::ChecksumFramer;
    let payload = b"abc";
    let mut out = Vec::new();
    let framer = ChecksumFramer::new(Crc32::new());
    framer.frame_and_write(&mut out, payload).unwrap();
    assert_eq!(out.len(), 4 + 4 + payload.len());
    let len = u32::from_le_bytes([out[0], out[1], out[2], out[3]]) as usize;
    assert_eq!(len, payload.len());
    let _cksum = u32::from_le_bytes(out[4..8].try_into().unwrap());
    assert_eq!(&out[8..], payload);
}

#[test]
fn nonstandard_checksum_width_is_byte_exact() {
    // Purpose: A custom width (3 bytes here) must produce a self-consistent,
    // byte-exact stream: exactly SIZE checksum bytes on the wire (the old
    // `_ =>` fallback silently wrote 8), verification modulo the wire width,
    // corruption still detected. Structural guarantee of the Checksum trait's
    // associated SIZE + write_bytes/read_bytes/verify defaults.
    use flatstream::checksum::Checksum;
    use flatstream::framing::{ChecksumDeframer, ChecksumFramer, Deframer, Framer};

    /// Toy 3-byte checksum: byte sum, deliberately *wider* than 3 bytes
    /// before truncation to exercise the width-masked verification.
    #[derive(Clone, Copy, Default)]
    struct Sum24;
    impl Checksum for Sum24 {
        const SIZE: usize = 3;
        fn calculate(&self, payload: &[u8]) -> u64 {
            payload.iter().map(|&b| b as u64).sum::<u64>() | 0xFF00_0000_0000_0000
        }
    }

    let payload = b"nonstandard width";
    let mut out = Vec::new();
    ChecksumFramer::new(Sum24)
        .frame_and_write(&mut out, payload)
        .unwrap();

    // Layout: [4-byte LE length | 3-byte LE checksum | payload]
    assert_eq!(out.len(), 4 + 3 + payload.len());
    let len = u32::from_le_bytes(out[..4].try_into().unwrap()) as usize;
    assert_eq!(len, payload.len());
    let wire_sum = u32::from_le_bytes([out[4], out[5], out[6], 0]) as u64;
    assert_eq!(
        wire_sum,
        payload.iter().map(|&b| b as u64).sum::<u64>() & 0x00FF_FFFF
    );
    assert_eq!(&out[7..], payload);

    // Roundtrip verifies.
    let deframer = ChecksumDeframer::new(Sum24);
    let mut buf = Vec::new();
    let n = deframer
        .read_and_deframe(&mut std::io::Cursor::new(&out), &mut buf)
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], payload);

    // Corruption within the low 3 bytes is still detected.
    let mut corrupt = out.clone();
    corrupt[7] ^= 0x01; // first payload byte
    let mut buf = Vec::new();
    let err = deframer
        .read_and_deframe(&mut std::io::Cursor::new(&corrupt), &mut buf)
        .unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::ChecksumMismatch { .. }));
}

#[cfg(feature = "crc16")]
#[test]
fn checksumframer_layout_crc16() {
    // Purpose: Verify ChecksumFramer::<Crc16> writes [4-byte LE length | 2-byte checksum | payload].
    use flatstream::framing::ChecksumFramer;
    let payload = b"abc";
    let mut out = Vec::new();
    let framer = ChecksumFramer::new(Crc16::new());
    framer.frame_and_write(&mut out, payload).unwrap();
    assert_eq!(out.len(), 4 + 2 + payload.len());
    let len = u32::from_le_bytes([out[0], out[1], out[2], out[3]]) as usize;
    assert_eq!(len, payload.len());
    let _cksum = u16::from_le_bytes(out[4..6].try_into().unwrap());
    assert_eq!(&out[6..], payload);
}
