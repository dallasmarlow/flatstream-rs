// tests/integration_tests.rs

use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use tempfile::NamedTempFile;

fn write_read_cycle<F, D>(framer: F, deframer: D, messages: &[String])
where
    F: Framer,
    D: Deframer,
{
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write messages
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let mut stream_writer = StreamWriter::new(writer, framer);

        // External builder management for zero-allocation writes
        let mut builder = FlatBufferBuilder::new();
        for msg in messages {
            builder.reset();
            let data = builder.create_string(&format!("message {i}"));
            builder.finish(data, None);
            stream_writer.write_finished(&mut builder).unwrap();
        }
        stream_writer.flush().unwrap();
    }

    // Read and validate
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let mut stream_reader = StreamReader::new(reader, deframer);

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

#[test]
fn test_write_read_cycle_default() {
    let msgs = (0..3).map(|i| format!("message {i}")).collect::<Vec<_>>();
    write_read_cycle(DefaultFramer, DefaultDeframer, &msgs);
}

#[test]
#[cfg(feature = "xxhash")]
fn test_write_read_cycle_with_checksum() {
    let msgs = vec!["important data".to_string()];
    write_read_cycle(
        ChecksumFramer::new(XxHash64::new()),
        ChecksumDeframer::new(XxHash64::new()),
        &msgs,
    );
}

#[test]
#[cfg(feature = "xxhash")]
fn test_corruption_detection_with_checksum() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write a message with checksum
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let framer = ChecksumFramer::new(XxHash64::new());
        let mut stream_writer = StreamWriter::new(writer, framer);

        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("important data");
        builder.finish(data, None);
        stream_writer.write_finished(&mut builder).unwrap();
        stream_writer.flush().unwrap();
    }

    // Corrupt the file by flipping a bit in the payload
    {
        let mut data = std::fs::read(path).unwrap();
        if !data.is_empty() {
            let last_byte_index = data.len() - 1;
            data[last_byte_index] ^= 1; // Flip the last bit of the payload
        }
        std::fs::write(path, data).unwrap();
    }

    // Try to read the corrupted file
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = ChecksumDeframer::new(XxHash64::new());
        let mut stream_reader = StreamReader::new(reader, deframer);

        let result = stream_reader.read_message();
        assert!(result.is_err());

        match result.unwrap_err() {
            Error::ChecksumMismatch { .. } => {
                // This is the expected outcome
            }
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

#[test]
#[cfg(feature = "crc32")]
fn test_write_read_cycle_with_crc32() {
    let msgs = vec!["crc32 test data".to_string()];
    write_read_cycle(
        ChecksumFramer::new(Crc32::new()),
        ChecksumDeframer::new(Crc32::new()),
        &msgs,
    );
}

#[test]
#[cfg(feature = "crc16")]
fn test_write_read_cycle_with_crc16() {
    let msgs = vec!["crc16 test data".to_string()];
    write_read_cycle(
        ChecksumFramer::new(Crc16::new()),
        ChecksumDeframer::new(Crc16::new()),
        &msgs,
    );
}

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
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write a message but truncate the file
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

    // Truncate the file to simulate corruption
    {
        let data = std::fs::read(path).unwrap();
        let truncated_size = data.len() - 5; // Remove last 5 bytes
        let mut file = File::create(path).unwrap();
        file.write_all(&data[..truncated_size]).unwrap();
    }

    // Try to read the truncated file using process_all
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let mut stream_reader = StreamReader::new(reader, deframer);

        let result = stream_reader.process_all(|_payload| Ok(()));
        assert!(result.is_err());

        match result.unwrap_err() {
            Error::UnexpectedEof => {} // Expected
            e => panic!("Expected UnexpectedEof error, got: {e:?}"),
        }
    }

    // Try to read the truncated file using messages().next()
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let mut stream_reader = StreamReader::new(reader, deframer);

        let mut messages = stream_reader.messages();
        let result = messages.next();
        assert!(result.is_err());

        match result.unwrap_err() {
            Error::UnexpectedEof => {} // Expected
            e => panic!("Expected UnexpectedEof error, got: {e:?}"),
        }
    }
}
