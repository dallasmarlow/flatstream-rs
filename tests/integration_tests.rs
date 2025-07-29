// tests/integration_tests.rs

use flatbuffers::FlatBufferBuilder;
use flatstream_rs::*;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use tempfile::NamedTempFile;

#[test]
fn test_write_read_cycle_default() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write messages with default framing
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut stream_writer = StreamWriter::new(writer, framer);

        // External builder management
        let mut builder = FlatBufferBuilder::new();

        for i in 0..3 {
            builder.reset();
            let data = builder.create_string(&format!("message {}", i));
            builder.finish(data, None);
            stream_writer.write(&mut builder).unwrap();
        }
        stream_writer.flush().unwrap();
    }

    // Read messages back with default deframer using processor API
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
#[cfg(feature = "xxhash")]
fn test_write_read_cycle_with_checksum() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write with checksum
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let framer = ChecksumFramer::new(XxHash64::new());
        let mut stream_writer = StreamWriter::new(writer, framer);

        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("important data");
        builder.finish(data, None);
        stream_writer.write(&mut builder).unwrap();
        stream_writer.flush().unwrap();
    }

    // Read back and verify using processor API
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = ChecksumDeframer::new(XxHash64::new());
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
        assert_eq!(count, 1);
    }
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
        stream_writer.write(&mut builder).unwrap();
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
            e => panic!("Expected ChecksumMismatch error, got: {:?}", e),
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
        stream_writer.write(&mut builder).unwrap();
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
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write with CRC32
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let framer = ChecksumFramer::new(Crc32::new());
        let mut stream_writer = StreamWriter::new(writer, framer);

        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("crc32 test data");
        builder.finish(data, None);
        stream_writer.write(&mut builder).unwrap();
        stream_writer.flush().unwrap();
    }

    // Read back and verify
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = ChecksumDeframer::new(Crc32::new());
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
        assert_eq!(count, 1);
    }
}

#[test]
#[cfg(feature = "crc16")]
fn test_write_read_cycle_with_crc16() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write with CRC16
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let framer = ChecksumFramer::new(Crc16::new());
        let mut stream_writer = StreamWriter::new(writer, framer);

        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("crc16 test data");
        builder.finish(data, None);
        stream_writer.write(&mut builder).unwrap();
        stream_writer.flush().unwrap();
    }

    // Read back and verify
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = ChecksumDeframer::new(Crc16::new());
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
        assert_eq!(count, 1);
    }
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
            stream_writer.write(&mut builder).unwrap();
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
            let data = builder.create_string(&format!("message {}", i));
            builder.finish(data, None);
            stream_writer.write(&mut builder).unwrap();
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
            stream_writer.write(&mut builder).unwrap();
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
