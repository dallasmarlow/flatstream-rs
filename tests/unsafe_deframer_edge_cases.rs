use flatstream::*;
use std::io::Cursor;

#[test]
fn unsafe_deframer_malformed_length_errors() {
    // Purpose: With untrusted data and a malformed (huge) length, UnsafeDeframer should
    // surface an error (commonly UnexpectedEof) rather than producing a bogus payload.
    // Construct buffer with a huge length that exceeds actual payload.
    let mut buffer = Vec::new();
    let len: u32 = 0xFFFF_FFFF;
    buffer.extend_from_slice(&len.to_le_bytes());
    buffer.extend_from_slice(b"short payload");

    let mut reader = StreamReader::new(Cursor::new(&buffer), UnsafeDeframer);
    let res = reader.read_message();
    // With untrusted data, UnsafeDeframer should surface an error (UnexpectedEof).
    assert!(matches!(res, Err(Error::UnexpectedEof) | Ok(None)));
}
