// tests/integration_tests.rs

use flatstream::*;
#[macro_use]
mod test_harness;
use test_harness::TestHarness;

test_framer_deframer_pair!(
    test_write_read_cycle_default,
    DefaultFramer,
    DefaultDeframer::new(),
    &((0..3).map(|i| format!("message {i}")).collect::<Vec<_>>())
);
// Purpose: Macro-driven test writes with the specified framer and reads with the
// specified deframer, asserting end-to-end roundtrip for the provided messages.

#[cfg(feature = "xxhash")]
test_framer_deframer_pair!(
    test_write_read_cycle_with_checksum,
    ChecksumFramer::new(XxHash64::new()),
    ChecksumDeframer::new(XxHash64::new()),
    &["important data".to_string()]
);
// Purpose: Validate end-to-end roundtrip with checksum-enabled framing/deframing.

#[test]
#[cfg(feature = "xxhash")]
fn test_corruption_detection_with_checksum() {
    // Purpose: After corrupting the on-disk data, checksum deframer should report mismatch.
    let harness = TestHarness::new();
    {
        let mut w = harness.writer(ChecksumFramer::new(XxHash64::new()));
        let mut b = flatbuffers::FlatBufferBuilder::new();
        let s = b.create_string("important data");
        b.finish(s, None);
        w.write_finished(&mut b).unwrap();
        w.flush().unwrap();
    }
    harness.corrupt_last_byte();
    {
        let mut r = harness.reader(ChecksumDeframer::new(XxHash64::new()));
        let err = r.read_message().unwrap_err();
        assert!(matches!(err.kind(), ErrorKind::ChecksumMismatch { .. }));
    }
}

#[test]
fn test_mismatched_framing_strategies() {
    // Purpose: Reading a default-framed stream with a checksum deframer should fail.
    let harness = TestHarness::new();
    {
        let mut w = harness.writer(DefaultFramer);
        let mut b = flatbuffers::FlatBufferBuilder::new();
        let s = b.create_string("a long partial message");
        b.finish(s, None);
        w.write_finished(&mut b).unwrap();
        w.flush().unwrap();
    }
    #[cfg(feature = "xxhash")]
    {
        let mut r = harness.reader(ChecksumDeframer::new(XxHash64::new()));
        let result = r.read_message();
        assert!(result.is_err());
    }
}

#[cfg(feature = "crc32")]
test_framer_deframer_pair!(
    test_write_read_cycle_with_crc32,
    ChecksumFramer::new(Crc32::new()),
    ChecksumDeframer::new(Crc32::new()),
    &["crc32 test data".to_string()]
);

#[cfg(feature = "crc16")]
test_framer_deframer_pair!(
    test_write_read_cycle_with_crc16,
    ChecksumFramer::new(Crc16::new()),
    ChecksumDeframer::new(Crc16::new()),
    &["crc16 test data".to_string()]
);

#[test]
fn test_large_stream_stress() {
    // Purpose: 1000-message sustained cycle with per-message content checks —
    // the high-water-mark buffer must reproduce every payload exactly across
    // many reuse cycles, not just count frames.
    let harness = TestHarness::new();
    let mut expected: Vec<Vec<u8>> = Vec::new();
    {
        let mut w = harness.writer(DefaultFramer);
        let mut b = flatbuffers::FlatBufferBuilder::new();
        for i in 0..1000 {
            b.reset();
            let s = b.create_string(&format!("message {i}"));
            b.finish(s, None);
            expected.push(b.finished_data().to_vec());
            w.write_finished(&mut b).unwrap();
        }
        w.flush().unwrap();
    }
    {
        let mut r = harness.reader(DefaultDeframer::new());
        let mut count = 0usize;
        r.process_all(|p| {
            assert_eq!(p, &expected[count][..]);
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 1000);
    }
}

#[test]
fn test_partial_file_read() {
    // Purpose: Truncating the file should surface UnexpectedEof in both process_all and messages().
    let harness = TestHarness::new();
    {
        let mut w = harness.writer(DefaultFramer);
        let mut b = flatbuffers::FlatBufferBuilder::new();
        let s = b.create_string("a long partial message");
        b.finish(s, None);
        w.write_finished(&mut b).unwrap();
        w.flush().unwrap();
    }
    harness.truncate_last_bytes(5);
    {
        let mut r = harness.reader(DefaultDeframer::new());
        let err = r.process_all(|_payload| Ok(())).unwrap_err();
        assert!(matches!(err.kind(), ErrorKind::UnexpectedEof));
    }
    {
        let mut r = harness.reader(DefaultDeframer::new());
        let mut msgs = r.messages();
        let err = msgs.next().unwrap_err();
        assert!(matches!(err.kind(), ErrorKind::UnexpectedEof));
    }
}
