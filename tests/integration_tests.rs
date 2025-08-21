// tests/integration_tests.rs

use flatbuffers::FlatBufferBuilder;
use flatstream::*;
mod test_harness;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use tempfile::NamedTempFile;
use test_harness::TestHarness;

#[allow(unused_macros)]
macro_rules! test_framer_deframer_pair {
    ($test_name:ident, $framer:expr, $deframer:expr, $messages:expr) => {
        #[test]
        fn $test_name() {
            write_read_cycle($framer, $deframer, $messages);
        }
    };
}

fn write_read_cycle<F, D>(framer: F, deframer: D, messages: &[String])
where
    F: Framer,
    D: Deframer,
{
    let harness = TestHarness::new();

    // Write messages
    {
        let mut stream_writer = harness.writer(framer);

        // External builder management for zero-allocation writes
        let mut builder = FlatBufferBuilder::new();
        for msg in messages {
            builder.reset();
            let data = builder.create_string(msg);
            builder.finish(data, None);
            stream_writer.write_finished(&mut builder).unwrap();
        }
        stream_writer.flush().unwrap();
    }

    // Read and validate
    {
        let mut stream_reader = harness.reader(deframer);

        let mut count = 0;
        stream_reader
            .process_all(|payload| {
                assert!(!payload.is_empty());
                count += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(count, messages.len());
    }
}

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
    // Write with checksum
    {
        let mut stream_writer = harness.writer(ChecksumFramer::new(XxHash64::new()));
        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("important data");
        builder.finish(data, None);
        stream_writer.write_finished(&mut builder).unwrap();
        stream_writer.flush().unwrap();
    }
    // Corrupt the last byte
    harness.corrupt_last_byte();
    // Read back expecting a checksum mismatch
    {
        let mut stream_reader = harness.reader(ChecksumDeframer::new(XxHash64::new()));
        let result = stream_reader.read_message();
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::ChecksumMismatch { .. } => {}
            e => panic!("Expected ChecksumMismatch error, got: {e:?}"),
        }
    }
}

#[test]
fn test_mismatched_framing_strategies() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write with default framing
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut stream_writer = StreamWriter::new(writer, framer);

        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("a long partial message");
        builder.finish(data, None);
        stream_writer.write_finished(&mut builder).unwrap();
        stream_writer.flush().unwrap();
    }

    // Try to read with checksum deframer (should fail gracefully)
    #[cfg(feature = "xxhash")]
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = ChecksumDeframer::new(XxHash64::new());
        let mut stream_reader = StreamReader::new(reader, deframer);

        let result = stream_reader.read_message();
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
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write various data types
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut stream_writer = StreamWriter::new(writer, framer);

        let mut builder = FlatBufferBuilder::new();
        let messages = vec![
            "short",
            "medium length message",
            "very long message with lots of content",
        ];

        for message in &messages {
            builder.reset();
            let data = builder.create_string(message);
            builder.finish(data, None);
            stream_writer.write_finished(&mut builder).unwrap();
        }
        stream_writer.flush().unwrap();
    }

    // Read back and verify
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let mut stream_reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        stream_reader
            .process_all(|payload| {
                // The payload contains FlatBuffer data, not the raw string
                // For this test, we just verify we got some data
                assert!(!payload.is_empty());
                count += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(count, 3);
    }
}

#[test]
fn test_large_stream_stress() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write many messages
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut stream_writer = StreamWriter::new(writer, framer);

        let mut builder = FlatBufferBuilder::new();
        for i in 0..1000 {
            builder.reset();
            let data = builder.create_string(&format!("message {i}"));
            builder.finish(data, None);
            stream_writer.write_finished(&mut builder).unwrap();
        }
        stream_writer.flush().unwrap();
    }

    // Read back and verify
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let mut stream_reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        stream_reader
            .process_all(|payload| {
                // The payload contains FlatBuffer data, not the raw string
                // For this test, we just verify we got some data
                assert!(!payload.is_empty());
                count += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(count, 1000);
    }
}

#[test]
fn test_realistic_telemetry_data() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write telemetry-like data
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut stream_writer = StreamWriter::new(writer, framer);

        let mut builder = FlatBufferBuilder::new();
        let telemetry_events = vec![
            "timestamp=1234567890,device_id=sensor-1,temperature=23.5,humidity=45.2",
            "timestamp=1234567891,device_id=sensor-2,temperature=24.1,humidity=46.8",
            "timestamp=1234567892,device_id=sensor-3,temperature=22.8,humidity=44.9",
        ];

        for event in &telemetry_events {
            builder.reset();
            let data = builder.create_string(event);
            builder.finish(data, None);
            stream_writer.write_finished(&mut builder).unwrap();
        }
        stream_writer.flush().unwrap();
    }

    // Read back and verify
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let mut stream_reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        stream_reader
            .process_all(|payload| {
                // The payload contains FlatBuffer data, not the raw string
                // For this test, we just verify we got some data
                assert!(!payload.is_empty());
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
    // Write one message
    {
        let mut stream_writer = harness.writer(DefaultFramer);
        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("a long partial message");
        builder.finish(data, None);
        stream_writer.write_finished(&mut builder).unwrap();
        stream_writer.flush().unwrap();
    }
    // Truncate last 5 bytes
    harness.truncate_last_bytes(5);
    // Read via process_all
    {
        let mut stream_reader = harness.reader(DefaultDeframer);
        let result = stream_reader.process_all(|_payload| Ok(()));
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::UnexpectedEof => {}
            e => panic!("Expected UnexpectedEof error, got: {e:?}"),
        }
    }
    // Read via messages().next()
    {
        let mut stream_reader = harness.reader(DefaultDeframer);
        let mut messages = stream_reader.messages();
        let result = messages.next();
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::UnexpectedEof => {}
            e => panic!("Expected UnexpectedEof error, got: {e:?}"),
        }
    }
}
