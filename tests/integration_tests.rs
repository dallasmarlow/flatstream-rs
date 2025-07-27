use flatbuffers::FlatBufferBuilder;
use flatstream_rs::{ChecksumType, Error, StreamReader, StreamWriter};
use std::fs::File;
use std::io::{BufReader, BufWriter, Cursor, Write};
use tempfile::NamedTempFile;

#[test]
fn test_write_read_cycle_with_checksum() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write messages
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let mut stream_writer = StreamWriter::new(writer, ChecksumType::XxHash64);

        for i in 0..3 {
            let mut builder = FlatBufferBuilder::new();
            let data = builder.create_string(&format!("message {}", i));
            builder.finish(data, None);
            stream_writer.write_message(&mut builder).unwrap();
        }
        stream_writer.flush().unwrap();
    }

    // Read messages back
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let stream_reader = StreamReader::new(reader, ChecksumType::XxHash64);

        let mut count = 0;
        for result in stream_reader {
            assert!(result.is_ok());
            count += 1;
        }

        assert_eq!(count, 3);
    }
}

#[test]
fn test_write_read_cycle_without_checksum() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write messages
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let mut stream_writer = StreamWriter::new(writer, ChecksumType::None);

        for i in 0..2 {
            let mut builder = FlatBufferBuilder::new();
            let data = builder.create_string(&format!("no checksum {}", i));
            builder.finish(data, None);
            stream_writer.write_message(&mut builder).unwrap();
        }
        stream_writer.flush().unwrap();
    }

    // Read messages back
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let stream_reader = StreamReader::new(reader, ChecksumType::None);

        let mut count = 0;
        for result in stream_reader {
            assert!(result.is_ok());
            count += 1;
        }

        assert_eq!(count, 2);
    }
}

#[test]
fn test_corruption_detection() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write a message with checksum
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let mut stream_writer = StreamWriter::new(writer, ChecksumType::XxHash64);

        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("important data");
        builder.finish(data, None);
        stream_writer.write_message(&mut builder).unwrap();
        stream_writer.flush().unwrap();
    }

    // Corrupt the file by flipping a bit
    {
        let mut data = std::fs::read(path).unwrap();
        if data.len() > 30 {
            data[30] ^= 1; // Flip a bit in the payload
        }
        std::fs::write(path, data).unwrap();
    }

    // Try to read the corrupted file
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let mut stream_reader = StreamReader::new(reader, ChecksumType::XxHash64);

        let result = stream_reader.read_message();
        assert!(result.is_err());

        match result.unwrap_err() {
            Error::ChecksumMismatch {
                expected,
                calculated,
            } => {
                assert_ne!(expected, calculated);
            }
            e => panic!("Expected ChecksumMismatch error, got: {:?}", e),
        }
    }
}

#[test]
fn test_large_stream() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write a large number of messages
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let mut stream_writer = StreamWriter::new(writer, ChecksumType::XxHash64);

        for i in 0..100 {
            let mut builder = FlatBufferBuilder::new();
            let data = builder.create_string(&format!("message number {}", i));
            builder.finish(data, None);
            stream_writer.write_message(&mut builder).unwrap();
        }
        stream_writer.flush().unwrap();
    }

    // Read all messages back
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let stream_reader = StreamReader::new(reader, ChecksumType::XxHash64);

        let mut count = 0;
        for result in stream_reader {
            assert!(result.is_ok());
            count += 1;
        }

        assert_eq!(count, 100);
    }
}

#[test]
fn test_empty_file() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Create an empty file
    File::create(path).unwrap();

    // Try to read from empty file
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let mut stream_reader = StreamReader::new(reader, ChecksumType::XxHash64);

        let result = stream_reader.read_message().unwrap();
        assert!(result.is_none());
    }
}

#[test]
fn test_partial_file() {
    let temp_file = NamedTempFile::new().unwrap();
    let path = temp_file.path();

    // Write a message but truncate the file
    {
        let file = File::create(path).unwrap();
        let writer = BufWriter::new(file);
        let mut stream_writer = StreamWriter::new(writer, ChecksumType::XxHash64);

        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("partial message");
        builder.finish(data, None);
        stream_writer.write_message(&mut builder).unwrap();
        stream_writer.flush().unwrap();
    }

    // Truncate the file to simulate corruption
    {
        let data = std::fs::read(path).unwrap();
        let truncated_size = data.len() - 10; // Remove last 10 bytes
        let mut file = File::create(path).unwrap();
        file.write_all(&data[..truncated_size]).unwrap();
    }

    // Try to read the truncated file
    {
        let file = File::open(path).unwrap();
        let reader = BufReader::new(file);
        let mut stream_reader = StreamReader::new(reader, ChecksumType::XxHash64);

        let result = stream_reader.read_message();
        assert!(result.is_err());

        match result.unwrap_err() {
            Error::UnexpectedEof => {} // Expected
            e => panic!("Expected UnexpectedEof error, got: {:?}", e),
        }
    }
}

#[test]
fn test_memory_stream() {
    let mut buffer = Vec::new();

    // Write to memory
    {
        let mut stream_writer = StreamWriter::new(Cursor::new(&mut buffer), ChecksumType::XxHash64);

        for i in 0..2 {
            let mut builder = FlatBufferBuilder::new();
            let data = builder.create_string(&format!("memory test {}", i));
            builder.finish(data, None);
            stream_writer.write_message(&mut builder).unwrap();
        }
    }

    // Read from memory
    {
        let stream_reader = StreamReader::new(Cursor::new(&buffer), ChecksumType::XxHash64);

        let mut count = 0;
        for result in stream_reader {
            assert!(result.is_ok());
            count += 1;
        }

        assert_eq!(count, 2);
    }
}

#[test]
fn test_mixed_checksum_types() {
    // Test that we can't read a file written without checksums using a reader with checksums
    let mut buffer = Vec::new();

    // Write without checksum
    {
        let mut stream_writer = StreamWriter::new(Cursor::new(&mut buffer), ChecksumType::None);

        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("no checksum");
        builder.finish(data, None);
        stream_writer.write_message(&mut builder).unwrap();
    }

    // Try to read with checksum (should fail due to format mismatch)
    {
        let mut stream_reader = StreamReader::new(Cursor::new(&buffer), ChecksumType::XxHash64);

        let result = stream_reader.read_message();
        assert!(result.is_err());

        match result.unwrap_err() {
            Error::UnexpectedEof => {} // Expected when trying to read checksum bytes that don't exist
            e => panic!("Expected UnexpectedEof error, got: {:?}", e),
        }
    }
}
