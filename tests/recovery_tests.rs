use flatstream::*;
use std::io::{Read, Seek, SeekFrom};

// The harness is shared with io_fault_injection.rs; this binary uses only
// the one-byte-chunks mode, so the other fault variants are dead code here.
#[allow(dead_code)]
mod harness {
    pub mod faulty_reader;
}
use harness::faulty_reader::{FaultMode, FaultyReader};

/// E1 contract, exhaustively: truncate a valid multi-frame stream at EVERY
/// byte offset and recover. Truncation only shortens — it never corrupts — so
/// every mid-frame cut is the crash signature (`UnexpectedEof` → `TornTail`)
/// and every boundary cut is `CleanEof`. The report must count exactly the
/// complete frames and point at the last frame boundary.
fn recovery_sweep<F: Framer, D: Deframer>(framer: &F, make_deframer: impl Fn() -> D) {
    let payloads: [&[u8]; 3] = [b"a", b"bb-bb", b"ccc-ccc-ccc"];
    let mut wire = Vec::new();
    let mut boundaries = vec![0u64];
    for p in payloads {
        framer.frame_and_write(&mut wire, p).unwrap();
        boundaries.push(wire.len() as u64);
    }

    for cut in 0..=wire.len() {
        let report = recover(&wire[..cut], make_deframer()).unwrap();
        let cut = cut as u64;
        let complete = boundaries.iter().filter(|&&b| b > 0 && b <= cut).count() as u64;
        let last_boundary = boundaries
            .iter()
            .copied()
            .filter(|&b| b <= cut)
            .max()
            .unwrap();
        let expected_end = if boundaries.contains(&cut) {
            RecoveryEnd::CleanEof
        } else {
            RecoveryEnd::TornTail
        };

        assert_eq!(report.frames, complete, "cut at {cut}");
        assert_eq!(report.last_good_offset, last_boundary, "cut at {cut}");
        assert_eq!(report.end, expected_end, "cut at {cut}");
    }
}

#[test]
fn recovery_truncation_sweep_default() {
    recovery_sweep(&DefaultFramer, DefaultDeframer::new);
}

#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
#[test]
fn recovery_truncation_sweep_checksummed() {
    #[cfg(feature = "xxhash")]
    recovery_sweep(&ChecksumFramer::new(XxHash64::new()), || {
        ChecksumDeframer::new(XxHash64::new())
    });
    #[cfg(feature = "crc32")]
    recovery_sweep(&ChecksumFramer::new(Crc32::new()), || {
        ChecksumDeframer::new(Crc32::new())
    });
    #[cfg(feature = "crc16")]
    recovery_sweep(&ChecksumFramer::new(Crc16::new()), || {
        ChecksumDeframer::new(Crc16::new())
    });
}

#[test]
fn recovery_of_empty_stream_is_clean() {
    // Zero bytes at a frame boundary is clean EOF (spec §6): nothing to do.
    let report = recover(std::io::empty(), DefaultDeframer::new()).unwrap();
    assert_eq!(
        report,
        RecoveryReport {
            frames: 0,
            last_good_offset: 0,
            end: RecoveryEnd::CleanEof,
        }
    );
}

#[cfg(feature = "xxhash")]
#[test]
fn recovery_errors_on_corrupted_payload() {
    // A checksummed journal where frame 2 of 3 took a bit flip. That is
    // corruption inside a fully present frame — NOT a torn append — and a
    // valid frame 3 follows it. Truncation would destroy that valid suffix,
    // so recovery must surface the mismatch as an error, not a result.
    let framer = ChecksumFramer::new(XxHash64::new());
    let mut wire = Vec::new();
    framer.frame_and_write(&mut wire, b"good frame").unwrap();
    let boundary = wire.len();
    framer.frame_and_write(&mut wire, b"doomed frame").unwrap();
    framer.frame_and_write(&mut wire, b"valid suffix").unwrap();
    let flip_at = boundary + 4 + 8 + 2; // inside frame 2's payload
    wire[flip_at] ^= 0x01;

    let err = recover(wire.as_slice(), ChecksumDeframer::new(XxHash64::new())).unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::ChecksumMismatch { .. }));
}

#[test]
fn recovery_errors_on_validation_failure() {
    // A validator rejecting an intact frame is read-policy (schema/validator
    // drift), not a torn journal: recovery must not offer a truncation point.
    // Recovery runs with the deframer that matches the wire format — plain,
    // no validators; this pins what happens when that advice is ignored.
    let mut wire = Vec::new();
    DefaultFramer.frame_and_write(&mut wire, b"ok").unwrap();
    DefaultFramer
        .frame_and_write(&mut wire, b"this payload is too large")
        .unwrap();

    let deframer = DefaultDeframer::new().with_validator(SizeValidator::new(0, 8));
    let err = recover(wire.as_slice(), deframer).unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::ValidationFailed { .. }));
}

#[test]
fn recovery_errors_on_configured_bound_mismatch() {
    // A bound smaller than the stream's real frames is a configuration
    // error, not a torn tail: the frame is intact, the reader is wrong.
    let mut wire = Vec::new();
    DefaultFramer.frame_and_write(&mut wire, b"ok").unwrap();
    DefaultFramer
        .frame_and_write(&mut wire, b"sixteen byte body")
        .unwrap();

    let deframer = DefaultDeframer::new().with_max_frame_len(8);
    let err = recover(wire.as_slice(), deframer).unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::InvalidFrame { .. }));
}

#[test]
fn recovery_errors_on_complete_oversize_header_at_eof() {
    // A complete four-byte length above the configured ceiling is not a torn
    // header: it is an invalid frame declaration. The valid prefix remains
    // untouched, but strict crash recovery returns Err rather than offering a
    // truncation report; explicit salvage is a separate operation.
    let mut wire = Vec::new();
    DefaultFramer
        .frame_and_write(&mut wire, b"intact prefix")
        .unwrap();
    wire.extend_from_slice(&u32::MAX.to_le_bytes());

    let err = recover(wire.as_slice(), DefaultDeframer::new()).unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::InvalidFrame { .. }));
}

#[cfg(feature = "xxhash")]
#[test]
fn recovery_errors_on_wrong_checksum_configuration() {
    // A checksummed deframer scanning a plain-framed stream misparses: it
    // consumes payload bytes as a checksum field and the verification fails.
    // That mismatch must surface as Err — a wrong reader configuration must
    // never authorize truncating a perfectly good file. (The opposite
    // direction — a plain deframer over a checksummed stream — silently
    // mis-frames and cannot be detected structurally in v2; that is the
    // documented limitation the Format v3 self-describing header closes. See
    // tests/wire_format_corpus.rs for the pinned misframe semantics.)
    let mut wire = Vec::new();
    DefaultFramer
        .frame_and_write(&mut wire, b"frame one")
        .unwrap();
    DefaultFramer
        .frame_and_write(&mut wire, b"frame two")
        .unwrap();

    let err = recover(wire.as_slice(), ChecksumDeframer::new(XxHash64::new())).unwrap_err();
    assert!(matches!(err.kind(), ErrorKind::ChecksumMismatch { .. }));
}

#[test]
fn recovery_is_exact_under_one_byte_reads() {
    // Short reads must not change the verdict: a reader that delivers one
    // byte per call over an intact-plus-torn stream produces the identical
    // report to a direct scan.
    let mut wire = Vec::new();
    DefaultFramer
        .frame_and_write(&mut wire, b"frame one")
        .unwrap();
    DefaultFramer
        .frame_and_write(&mut wire, b"frame two")
        .unwrap();
    let intact = wire.len() as u64;
    wire.extend_from_slice(&[9, 0, 0, 0, b'x', b'y']); // torn tail

    let direct = recover(wire.as_slice(), DefaultDeframer::new()).unwrap();
    let chunked = recover(
        FaultyReader::new(wire.as_slice(), FaultMode::OneByteChunks),
        DefaultDeframer::new(),
    )
    .unwrap();
    assert_eq!(direct, chunked);
    assert_eq!(direct.frames, 2);
    assert_eq!(direct.last_good_offset, intact);
    assert_eq!(direct.end, RecoveryEnd::TornTail);
}

#[test]
fn recovery_is_exact_under_interrupted_reads() {
    // `Interrupted` is retried inside the read loops (spec §6 / B2), so a
    // reader that fails every third call with EINTR must produce the
    // identical report — interruption is neither a torn tail nor a device
    // fault.
    let mut wire = Vec::new();
    DefaultFramer
        .frame_and_write(&mut wire, b"frame one")
        .unwrap();
    DefaultFramer
        .frame_and_write(&mut wire, b"frame two")
        .unwrap();
    let intact = wire.len() as u64;
    wire.extend_from_slice(&[9, 0, 0, 0, b'x', b'y']); // torn tail

    let direct = recover(wire.as_slice(), DefaultDeframer::new()).unwrap();
    let interrupted = recover(
        FaultyReader::new(wire.as_slice(), FaultMode::InterruptedEvery(3)),
        DefaultDeframer::new(),
    )
    .unwrap();
    assert_eq!(direct, interrupted);
    assert_eq!(direct.frames, 2);
    assert_eq!(direct.last_good_offset, intact);
    assert_eq!(direct.end, RecoveryEnd::TornTail);
}

#[test]
fn recover_scans_from_current_position_with_relative_offsets() {
    // `recover` starts wherever the reader is and reports offsets relative
    // to that position — documented behavior, pinned here.
    let mut wire = Vec::new();
    DefaultFramer.frame_and_write(&mut wire, b"first").unwrap();
    let skip = wire.len() as u64;
    DefaultFramer.frame_and_write(&mut wire, b"second").unwrap();
    let second_len = wire.len() as u64 - skip;

    let mut cursor = std::io::Cursor::new(&wire);
    cursor.seek(SeekFrom::Start(skip)).unwrap();
    let report = recover(&mut cursor, DefaultDeframer::new()).unwrap();
    assert_eq!(report.frames, 1);
    assert_eq!(report.last_good_offset, second_len); // relative to `skip`
    assert_eq!(report.end, RecoveryEnd::CleanEof);
}

#[test]
fn recover_file_is_absolute_regardless_of_initial_cursor() {
    // `recover_file` seeks to the start itself, reports absolute offsets,
    // and leaves the cursor at `last_good_offset`, ready to resume.
    let mut wire = Vec::new();
    DefaultFramer.frame_and_write(&mut wire, b"first").unwrap();
    DefaultFramer.frame_and_write(&mut wire, b"second").unwrap();
    let intact = wire.len() as u64;
    wire.extend_from_slice(&[42, 0, 0, 0]); // torn: header only

    let mut cursor = std::io::Cursor::new(wire);
    cursor.seek(SeekFrom::End(-1)).unwrap(); // arbitrary nonzero position
    let report = recover_file(&mut cursor, DefaultDeframer::new()).unwrap();
    assert_eq!(report.frames, 2);
    assert_eq!(report.last_good_offset, intact);
    assert_eq!(report.end, RecoveryEnd::TornTail);
    assert_eq!(cursor.stream_position().unwrap(), intact);
}

/// Yields the prefix of an intact stream, then fails with a non-EOF device
/// fault. Recovery must NOT report a truncation point — truncating a journal
/// because the disk faulted mid-scan would destroy good data.
struct FaultingReader<'a> {
    data: &'a [u8],
    pos: usize,
    fail_at: usize,
}

impl Read for FaultingReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.fail_at {
            return Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        }
        let n = buf
            .len()
            .min(self.fail_at - self.pos)
            .min(self.data.len() - self.pos);
        buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

#[test]
fn recovery_propagates_device_faults() {
    let mut wire = Vec::new();
    DefaultFramer
        .frame_and_write(&mut wire, b"frame one")
        .unwrap();
    DefaultFramer
        .frame_and_write(&mut wire, b"frame two")
        .unwrap();

    let reader = FaultingReader {
        data: &wire,
        pos: 0,
        fail_at: wire.len() - 3, // fault strikes inside frame 2
    };
    let err = recover(reader, DefaultDeframer::new()).unwrap_err();
    match err.kind() {
        ErrorKind::Io(io) => assert_eq!(io.kind(), std::io::ErrorKind::PermissionDenied),
        other => panic!("expected Io(PermissionDenied), got {other:?}"),
    }
}

#[test]
fn recovery_then_resume_append_roundtrips_byte_exact() {
    // The full journal story: crash leaves a torn tail, recover_file,
    // truncate ONLY on TornTail, append from the repaired end, and a normal
    // read yields every payload byte-identically.
    let mut journal = Vec::new();
    DefaultFramer
        .frame_and_write(&mut journal, b"before-1")
        .unwrap();
    DefaultFramer
        .frame_and_write(&mut journal, b"before-2")
        .unwrap();
    journal.extend_from_slice(&[200, 0, 0, 0, 0xAB, 0xCD]); // torn append

    let mut file = std::io::Cursor::new(journal);
    let report = recover_file(&mut file, DefaultDeframer::new()).unwrap();
    assert_eq!(report.frames, 2);
    assert_eq!(report.end, RecoveryEnd::TornTail);

    if report.end == RecoveryEnd::TornTail {
        let offset =
            usize::try_from(report.last_good_offset).expect("in-memory journal offset fits usize");
        file.get_mut().truncate(offset);
    }
    assert_eq!(file.stream_position().unwrap(), report.last_good_offset);
    DefaultFramer
        .frame_and_write(&mut file, b"after-1")
        .unwrap();

    let journal = file.into_inner();
    let mut seen: Vec<Vec<u8>> = Vec::new();
    let mut reader = StreamReader::new(std::io::Cursor::new(&journal), DefaultDeframer::new());
    reader
        .process_all(|p| {
            seen.push(p.to_vec());
            Ok(())
        })
        .unwrap();
    assert_eq!(seen, [&b"before-1"[..], b"before-2", b"after-1"]);

    // The repaired journal now recovers clean — and CleanEof means no
    // truncation is performed at all.
    let mut file = std::io::Cursor::new(journal);
    let report = recover_file(&mut file, DefaultDeframer::new()).unwrap();
    assert_eq!(report.frames, 3);
    assert_eq!(report.end, RecoveryEnd::CleanEof);
}
