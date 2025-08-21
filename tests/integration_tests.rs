// tests/integration_tests.rs

use flatstream::*;
#[macro_use]
mod test_harness;
use test_harness::TestHarness;

test_framer_deframer_pair!(
    test_write_read_cycle_default,
    DefaultFramer,
    DefaultDeframer,
    &((0..3).map(|i| format!("message {i}")).collect::<Vec<_>>())
);

#[cfg(feature = "xxhash")]
test_framer_deframer_pair!(
    test_write_read_cycle_with_checksum,
    ChecksumFramer::new(XxHash64::new()),
    ChecksumDeframer::new(XxHash64::new()),
    &["important data".to_string()]
);

#[test]
#[cfg(feature = "xxhash")]
fn test_corruption_detection_with_checksum() {
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
        let result = r.read_message();
        assert!(matches!(result, Err(Error::ChecksumMismatch { .. })));
    }
}

#[test]
fn test_mismatched_framing_strategies() {
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
fn test_comprehensive_data_types() {
    let harness = TestHarness::new();
    {
        let mut w = harness.writer(DefaultFramer);
        let mut b = flatbuffers::FlatBufferBuilder::new();
        let messages = [
            "short",
            "medium length message",
            "very long message with lots of content",
        ];
        for m in messages {
            b.reset();
            let s = b.create_string(m);
            b.finish(s, None);
            w.write_finished(&mut b).unwrap();
        }
        w.flush().unwrap();
    }
    {
        let mut r = harness.reader(DefaultDeframer);
        let mut count = 0usize;
        r.process_all(|p| {
            assert!(!p.is_empty());
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 3);
    }
}

#[test]
fn test_large_stream_stress() {
    let harness = TestHarness::new();
    {
        let mut w = harness.writer(DefaultFramer);
        let mut b = flatbuffers::FlatBufferBuilder::new();
        for i in 0..1000 {
            b.reset();
            let s = b.create_string(&format!("message {i}"));
            b.finish(s, None);
            w.write_finished(&mut b).unwrap();
        }
        w.flush().unwrap();
    }
    {
        let mut r = harness.reader(DefaultDeframer);
        let mut count = 0usize;
        r.process_all(|p| {
            assert!(!p.is_empty());
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 1000);
    }
}

#[test]
fn test_realistic_telemetry_data() {
    let harness = TestHarness::new();
    {
        let mut w = harness.writer(DefaultFramer);
        let mut b = flatbuffers::FlatBufferBuilder::new();
        let telemetry_events = [
            "timestamp=1234567890,device_id=sensor-1,temperature=23.5,humidity=45.2",
            "timestamp=1234567891,device_id=sensor-2,temperature=24.1,humidity=46.8",
            "timestamp=1234567892,device_id=sensor-3,temperature=22.8,humidity=44.9",
        ];
        for e in telemetry_events {
            b.reset();
            let s = b.create_string(e);
            b.finish(s, None);
            w.write_finished(&mut b).unwrap();
        }
        w.flush().unwrap();
    }
    {
        let mut r = harness.reader(DefaultDeframer);
        let mut count = 0usize;
        r.process_all(|p| {
            assert!(!p.is_empty());
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 3);
    }
}

#[test]
fn test_partial_file_read() {
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
        let mut r = harness.reader(DefaultDeframer);
        let result = r.process_all(|_payload| Ok(()));
        assert!(matches!(result, Err(Error::UnexpectedEof)));
    }
    {
        let mut r = harness.reader(DefaultDeframer);
        let mut msgs = r.messages();
        let result = msgs.next();
        assert!(matches!(result, Err(Error::UnexpectedEof)));
    }
}
