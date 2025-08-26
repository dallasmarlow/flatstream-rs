use flatstream::*;

#[test]
fn oversized_length_returns_invalid_frame_or_eof() {
    // Purpose: Increasing the length prefix beyond the actual payload should cause
    // a read failure (commonly UnexpectedEof when attempting to read past the end).
    // Build a frame with claimed length larger than actual payload, using default framing
    let mut out = Vec::new();
    DefaultFramer.frame_and_write(&mut out, b"hello").unwrap();
    // Bump the 4-byte little-endian length to something larger than the payload
    let mut len = u32::from_le_bytes([out[0], out[1], out[2], out[3]]) as usize;
    len += 10;
    let len_bytes = (len as u32).to_le_bytes();
    out[0..4].copy_from_slice(&len_bytes);

    let mut reader = StreamReader::new(std::io::Cursor::new(out), DefaultDeframer);
    let result = reader.read_message();
    // Depending on implementation, this can surface as UnexpectedEof due to short read
    assert!(matches!(result, Err(Error::UnexpectedEof)));
}

#[cfg(feature = "xxhash")]
#[test]
fn corrupted_checksum_region_returns_mismatch() {
    // Purpose: Corrupting bytes inside the checksum region must be detected and surfaced
    // as a ChecksumMismatch when using the checksum deframer.
    // Build a valid checksummed frame
    let mut out = Vec::new();
    let framer = ChecksumFramer::new(XxHash64::new());
    framer.frame_and_write(&mut out, b"payload").unwrap();

    // Corrupt a byte in the checksum region (bytes 4..12 for xxhash64)
    if out.len() >= 12 {
        out[8] ^= 0xFF;
    }

    let mut reader = StreamReader::new(
        std::io::Cursor::new(out),
        ChecksumDeframer::new(XxHash64::new()),
    );
    let result = reader.read_message();
    assert!(matches!(result, Err(Error::ChecksumMismatch { .. })));
}
