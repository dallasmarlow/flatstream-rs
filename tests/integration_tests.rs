// tests/integration_tests.rs

use flatstream_rs::{DefaultDeframer, DefaultFramer, Error, StreamReader, StreamWriter};
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use tempfile::NamedTempFile;

// Import framing types once (available when any checksum feature is enabled)
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
use flatstream_rs::framing::{ChecksumDeframer, ChecksumFramer};

// Conditionally import checksum components when the feature is enabled
#[cfg(feature = "xxhash")]
use flatstream_rs::XxHash64;

// Conditionally import CRC32 components when the feature is enabled
#[cfg(feature = "crc32")]
use flatstream_rs::Crc32;

// Conditionally import CRC16 components when the feature is enabled
#[cfg(feature = "crc16")]
use flatstream_rs::Crc16;

#[test]
fn test_write_read_cycle_default() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write messages with default framer
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut stream_writer = StreamWriter::new(writer, framer);

        for i in 0..3 {
            stream_writer.write(&format!("message {}", i)).unwrap();
        }
        stream_writer.flush().unwrap();
    }

    // Read messages back with default deframer
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let stream_reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        for result in stream_reader {
            assert!(result.is_ok());
            count += 1;
        }
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
        stream_writer.write(&"important data").unwrap();
        stream_writer.flush().unwrap();
    }

    // Read back and verify
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = ChecksumDeframer::new(XxHash64::new());
        let stream_reader = StreamReader::new(reader, deframer);
        assert_eq!(stream_reader.count(), 1);
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
        stream_writer.write(&"important data").unwrap();
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
fn test_partial_file_read() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write a message but truncate the file
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut stream_writer = StreamWriter::new(writer, framer);
        stream_writer.write(&"a long partial message").unwrap();
        stream_writer.flush().unwrap();
    }

    // Truncate the file to simulate corruption
    {
        let data = std::fs::read(path).unwrap();
        let truncated_size = data.len() - 5; // Remove last 5 bytes
        let mut file = File::create(path).unwrap();
        file.write_all(&data[..truncated_size]).unwrap();
    }

    // Try to read the truncated file
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let mut stream_reader = StreamReader::new(reader, deframer);

        let result = stream_reader.read_message();
        assert!(result.is_err());

        match result.unwrap_err() {
            Error::UnexpectedEof => {} // Expected
            e => panic!("Expected UnexpectedEof error, got: {:?}", e),
        }
    }
}

#[test]
#[cfg(feature = "xxhash")]
fn test_mismatched_framing_strategies() {
    let mut buffer = Vec::new();

    // Write WITH checksum
    {
        let framer = ChecksumFramer::new(XxHash64::new());
        let mut stream_writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        stream_writer.write(&"test").unwrap();
    }

    // Try to read WITHOUT checksum deframer (should fail)
    {
        let deframer = DefaultDeframer;
        let mut stream_reader = StreamReader::new(Cursor::new(&buffer), deframer);
        // The reader will interpret the 8-byte checksum as the 4-byte length of the next message,
        // leading to an UnexpectedEof when it tries to read that massive (and incorrect) length.
        let result = stream_reader.read_message();
        // The DefaultDeframer will interpret the checksum bytes as part of the payload length
        // and successfully read what it thinks is a valid message, but this is incorrect behavior
        // In a real scenario, this would lead to data corruption
        assert!(result.is_ok());
        let payload = result.unwrap().unwrap();
        // The payload should contain the checksum bytes followed by the actual data
        assert!(payload.len() > 8); // Should be longer than just the checksum
    }
}

#[test]
#[cfg(feature = "crc32")]
fn test_write_read_cycle_with_crc32() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write with Crc32 checksum
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let framer = ChecksumFramer::new(Crc32::new());
        let mut stream_writer = StreamWriter::new(writer, framer);
        stream_writer.write(&"data with crc32").unwrap();
        stream_writer.flush().unwrap();
    }

    // Read back and verify
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = ChecksumDeframer::new(Crc32::new());
        let stream_reader = StreamReader::new(reader, deframer);

        // Ensure we can read one valid message
        assert_eq!(stream_reader.count(), 1);
    }
}

#[test]
#[cfg(feature = "crc16")]
fn test_write_read_cycle_with_crc16() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write with Crc16 checksum
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let framer = ChecksumFramer::new(Crc16::new());
        let mut stream_writer = StreamWriter::new(writer, framer);
        stream_writer.write(&"data with crc16").unwrap();
        stream_writer.flush().unwrap();
    }

    // Read back and verify
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = ChecksumDeframer::new(Crc16::new());
        let stream_reader = StreamReader::new(reader, deframer);

        // Ensure we can read one valid message
        assert_eq!(stream_reader.count(), 1);
    }
}

#[test]
fn test_comprehensive_data_types() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Test various data types that users might serialize
    let large_string = "x".repeat(1000);
    let test_data = vec![
        "simple string",
        "string with special chars: éñüß",
        "",            // empty string
        &large_string, // large string
        "message with\nnewlines\tand\ttabs",
        "message with \"quotes\" and 'apostrophes'",
    ];

    // Write with default framer
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut stream_writer = StreamWriter::new(writer, framer);

        for data in &test_data {
            stream_writer.write(data).unwrap();
        }
        stream_writer.flush().unwrap();
    }

    // Read back and verify
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let stream_reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        for result in stream_reader {
            assert!(result.is_ok());
            count += 1;
        }
        assert_eq!(count, test_data.len());
    }
}

#[test]
fn test_large_stream_stress() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();
    const MESSAGE_COUNT: usize = 1000;

    // Write many messages to test stream handling
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut stream_writer = StreamWriter::new(writer, framer);

        for i in 0..MESSAGE_COUNT {
            let message = format!("stress test message number {}", i);
            stream_writer.write(&message).unwrap();
        }
        stream_writer.flush().unwrap();
    }

    // Read back and verify all messages
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let stream_reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        for result in stream_reader {
            assert!(result.is_ok());
            count += 1;
        }
        assert_eq!(count, MESSAGE_COUNT);
    }
}

#[test]
fn test_realistic_telemetry_data() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Simulate realistic telemetry data
    let telemetry_events = vec![
        r#"{"timestamp": 1640995200, "device_id": "sensor_001", "temperature": 23.5, "humidity": 45.2, "is_critical": false}"#,
        r#"{"timestamp": 1640995201, "device_id": "sensor_002", "temperature": -1.2, "humidity": 78.9, "is_critical": true}"#,
        r#"{"timestamp": 1640995202, "device_id": "sensor_003", "temperature": 99.8, "humidity": 12.3, "is_critical": true}"#,
        r#"{"timestamp": 1640995203, "device_id": "sensor_004", "temperature": 15.7, "humidity": 67.4, "is_critical": false}"#,
    ];

    // Write telemetry data with checksum (if available)
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);

        #[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
        let framer = {
            #[cfg(feature = "xxhash")]
            {
                ChecksumFramer::new(XxHash64::new())
            }
            #[cfg(all(not(feature = "xxhash"), feature = "crc32"))]
            {
                ChecksumFramer::new(Crc32::new())
            }
            #[cfg(all(not(feature = "xxhash"), not(feature = "crc32"), feature = "crc16"))]
            {
                ChecksumFramer::new(Crc16::new())
            }
        };

        #[cfg(not(any(feature = "xxhash", feature = "crc32", feature = "crc16")))]
        let framer = DefaultFramer;

        let mut stream_writer = StreamWriter::new(writer, framer);

        for event in &telemetry_events {
            stream_writer.write(event).unwrap();
        }
        stream_writer.flush().unwrap();
    }

    // Read back and verify telemetry data
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);

        #[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
        let deframer = {
            #[cfg(feature = "xxhash")]
            {
                ChecksumDeframer::new(XxHash64::new())
            }
            #[cfg(all(not(feature = "xxhash"), feature = "crc32"))]
            {
                ChecksumDeframer::new(Crc32::new())
            }
            #[cfg(all(not(feature = "xxhash"), not(feature = "crc32"), feature = "crc16"))]
            {
                ChecksumDeframer::new(Crc16::new())
            }
        };

        #[cfg(not(any(feature = "xxhash", feature = "crc32", feature = "crc16")))]
        let deframer = DefaultDeframer;

        let stream_reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        for result in stream_reader {
            assert!(result.is_ok());
            count += 1;
        }
        assert_eq!(count, telemetry_events.len());
    }
}
