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

    let mut reader = StreamReader::new(std::io::Cursor::new(out), DefaultDeframer::new());
    // The declared length exceeds the bytes available: the payload read hits EOF.
    let err = reader.read_message().unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::UnexpectedEof));
}

/// WIRE_FORMAT_SPEC.md §6, exhaustively: truncate a valid multi-frame stream at
/// EVERY byte offset. Complete frames must be yielded intact and in order; then
/// the cut is a clean EOF if and only if it falls exactly on a frame boundary,
/// otherwise `UnexpectedEof`. Never a wrong payload, never a panic. This
/// subsumes the hand-picked torn-header and torn-checksum-field cases.
fn truncation_sweep<F: Framer, D: Deframer>(framer: &F, make_deframer: impl Fn() -> D) {
    let payloads: [&[u8]; 3] = [b"a", b"bb-bb", b"ccc-ccc-ccc"];
    let mut wire = Vec::new();
    let mut boundaries = vec![0usize];
    for p in payloads {
        framer.frame_and_write(&mut wire, p).unwrap();
        boundaries.push(wire.len());
    }

    for cut in 0..=wire.len() {
        let mut reader = StreamReader::new(std::io::Cursor::new(&wire[..cut]), make_deframer());
        let complete = boundaries.iter().filter(|&&b| b > 0 && b <= cut).count();
        for expected in payloads.iter().take(complete) {
            let payload = reader.read_message().unwrap().expect("complete frame");
            assert_eq!(payload, *expected, "cut at {cut}");
        }
        if boundaries.contains(&cut) {
            assert!(
                reader.read_message().unwrap().is_none(),
                "cut at frame boundary {cut} must be clean EOF"
            );
        } else {
            let err = reader.read_message().unwrap_err();
            assert!(
                matches!(err.kind(), ErrorKind::UnexpectedEof),
                "cut inside a frame at {cut} must be UnexpectedEof, got {err:?}"
            );
        }
    }
}

#[test]
fn truncation_sweep_default() {
    truncation_sweep(&DefaultFramer, DefaultDeframer::new);
}

#[cfg(feature = "xxhash")]
#[test]
fn truncation_sweep_checksummed() {
    // Covers every cut inside the merged [len | checksum] header as well as
    // the payload region.
    truncation_sweep(&ChecksumFramer::new(XxHash64::new()), || {
        ChecksumDeframer::new(XxHash64::new())
    });
}

#[test]
fn default_bound_rejects_hostile_length_before_allocation() {
    // Purpose: A corrupt header demanding ~4 GiB must be rejected by the default
    // max_frame_len (16 MiB) with InvalidFrame — before any allocation is sized
    // from attacker-controlled input.
    let mut data = u32::MAX.to_le_bytes().to_vec();
    data.extend_from_slice(b"short");

    let mut reader = StreamReader::new(std::io::Cursor::new(data), DefaultDeframer::new());
    let err = reader.read_message().unwrap_err();
    match err.into_kind() {
        ErrorKind::InvalidFrame {
            declared_len,
            limit,
            ..
        } => {
            assert_eq!(declared_len, Some(u32::MAX as usize));
            assert_eq!(limit, Some(DEFAULT_MAX_FRAME_LEN));
        }
        other => panic!("expected InvalidFrame, got {other:?}"),
    }
}

#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
#[test]
fn checksum_default_bound_rejects_hostile_length_before_allocation() {
    use flatstream::checksum::Checksum;

    fn check<C: Checksum>(checksum: C) {
        // The checksummed reader consumes the fixed-size checksum field as part
        // of its merged header, then must reject the hostile payload length
        // before sizing the payload buffer.
        let mut data = vec![0u8; 4 + C::SIZE];
        data[..4].copy_from_slice(&u32::MAX.to_le_bytes());

        let mut reader =
            StreamReader::new(std::io::Cursor::new(data), ChecksumDeframer::new(checksum));
        let err = reader.read_message().unwrap_err();
        match err.into_kind() {
            ErrorKind::InvalidFrame {
                declared_len,
                limit,
                ..
            } => {
                assert_eq!(declared_len, Some(u32::MAX as usize));
                assert_eq!(limit, Some(DEFAULT_MAX_FRAME_LEN));
            }
            other => panic!("expected InvalidFrame, got {other:?}"),
        }
    }

    #[cfg(feature = "xxhash")]
    check(XxHash64::new());
    #[cfg(feature = "crc32")]
    check(Crc32::new());
    #[cfg(feature = "crc16")]
    check(Crc16::new());
}

#[test]
fn unbounded_deframer_accepts_over_default_limit() {
    // Purpose: `unbounded()` is the explicit opt-out — a frame larger than the
    // default bound roundtrips exactly.
    let payload = vec![0x5Au8; DEFAULT_MAX_FRAME_LEN + 1];
    let mut out = Vec::new();
    DefaultFramer.frame_and_write(&mut out, &payload).unwrap();

    // The default bound rejects it...
    let mut bounded = StreamReader::new(std::io::Cursor::new(&out), DefaultDeframer::new());
    let err = bounded.read_message().unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::InvalidFrame { .. }));

    // ...the explicit opt-in reads it back byte-identically.
    let mut unbounded = StreamReader::new(std::io::Cursor::new(&out), DefaultDeframer::unbounded());
    let read = unbounded.read_message().unwrap().unwrap();
    assert_eq!(read, payload);
}

#[test]
fn high_water_mark_buffer_yields_exact_payloads() {
    // Purpose: After a large message grows the internal buffer, a smaller
    // message must be yielded as exactly its own bytes — no stale suffix from
    // the high-water mark.
    let large = vec![0xABu8; 4096];
    let small = vec![0xCDu8; 16];
    let mut out = Vec::new();
    DefaultFramer.frame_and_write(&mut out, &large).unwrap();
    DefaultFramer.frame_and_write(&mut out, &small).unwrap();

    let mut reader = StreamReader::new(std::io::Cursor::new(out), DefaultDeframer::new());
    assert_eq!(reader.read_message().unwrap().unwrap(), &large[..]);
    assert_eq!(reader.read_message().unwrap().unwrap(), &small[..]);
    assert!(reader.read_message().unwrap().is_none());
}

#[cfg(feature = "xxhash")]
#[test]
fn checksum_field_io_error_propagates_intact() {
    // Purpose: A real I/O error (EIO-like) while reading the checksum field
    // must surface as ErrorKind::Io with its kind preserved — recovery logic
    // treats "torn frame, truncate" and "device fault, fail stop" differently,
    // so collapsing the kind to UnexpectedEof destroys that decision.
    use flatstream::framing::Deframer;
    use std::io::Read;

    /// Yields `good` bytes, then fails every read with a non-EOF error.
    struct FailAfter {
        good: Vec<u8>,
        pos: usize,
    }
    impl Read for FailAfter {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos < self.good.len() {
                let n = buf.len().min(self.good.len() - self.pos);
                buf[..n].copy_from_slice(&self.good[self.pos..self.pos + n]);
                self.pos += n;
                Ok(n)
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "injected device fault",
                ))
            }
        }
    }

    let mut frame = Vec::new();
    ChecksumFramer::new(XxHash64::new())
        .frame_and_write(&mut frame, b"payload")
        .unwrap();
    let deframer = ChecksumDeframer::new(XxHash64::new());

    // Merged-header path: fault lands inside the [len | checksum] header.
    let mut reader = FailAfter {
        good: frame[..5].to_vec(),
        pos: 0,
    };
    let mut buf = Vec::new();
    match deframer
        .read_and_deframe(&mut reader, &mut buf)
        .unwrap_err()
        .into_kind()
    {
        ErrorKind::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied),
        other => panic!("expected ErrorKind::Io, got {other:?}"),
    }

    // read_after_length path: length already consumed, fault in the checksum field.
    let mut reader = FailAfter {
        good: frame[4..6].to_vec(),
        pos: 0,
    };
    let mut buf = Vec::new();
    match deframer
        .read_after_length(&mut reader, &mut buf, b"payload".len())
        .unwrap_err()
        .into_kind()
    {
        ErrorKind::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::PermissionDenied),
        other => panic!("expected ErrorKind::Io, got {other:?}"),
    }
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
    let err = reader.read_message().unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::ChecksumMismatch { .. }));
}
