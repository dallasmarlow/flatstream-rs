use crate::checksum::ChecksumType;
use crate::error::{Error, Result};
use std::io::Read;

/// A reader for streaming FlatBuffers messages with optional checksum verification.
///
/// The `StreamReader` reads messages in the format:
/// `[4-byte Payload Length (u32, little-endian) | 8-byte Checksum (u64, little-endian, if enabled) | FlatBuffer Payload]`
pub struct StreamReader<R: Read> {
    reader: R,
    checksum_type: ChecksumType,
    buffer: Vec<u8>,
}

impl<R: Read> StreamReader<R> {
    /// Create a new `StreamReader` with the specified underlying reader and checksum type.
    ///
    /// # Arguments
    /// * `reader` - The underlying reader to read from
    /// * `checksum_type` - The type of checksum to use for data integrity verification
    pub fn new(reader: R, checksum_type: ChecksumType) -> Self {
        Self {
            reader,
            checksum_type,
            buffer: Vec::new(),
        }
    }

    /// Read the next message from the stream.
    ///
    /// This method:
    /// 1. Reads 4 bytes for payload length
    /// 2. Reads 8 bytes for checksum (if enabled)
    /// 3. Reads the payload bytes
    /// 4. Verifies the checksum
    /// 5. Returns the raw FlatBuffer payload
    ///
    /// # Returns
    /// * `Ok(Some(payload))` - Successfully read a message
    /// * `Ok(None)` - End of stream (clean EOF)
    /// * `Err(e)` - Error reading or verifying the message
    pub fn read_message(&mut self) -> Result<Option<Vec<u8>>> {
        // Read payload length (4 bytes, little-endian)
        let mut length_bytes = [0u8; 4];
        match self.reader.read_exact(&mut length_bytes) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Ok(None); // Clean EOF
            }
            Err(e) => return Err(Error::Io(e)),
        }

        let payload_length = u32::from_le_bytes(length_bytes) as usize;

        // Read checksum (8 bytes, little-endian, if enabled)
        let mut checksum_bytes = [0u8; 8];
        let expected_checksum = if self.checksum_type != ChecksumType::None {
            match self.reader.read_exact(&mut checksum_bytes) {
                Ok(_) => u64::from_le_bytes(checksum_bytes),
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Err(Error::UnexpectedEof);
                }
                Err(e) => return Err(Error::Io(e)),
            }
        } else {
            0 // Not used when checksums are disabled
        };

        // Read payload
        self.buffer.resize(payload_length, 0);
        match self.reader.read_exact(&mut self.buffer) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(Error::UnexpectedEof);
            }
            Err(e) => return Err(Error::Io(e)),
        }

        // Verify checksum
        self.checksum_type
            .verify_checksum(expected_checksum, &self.buffer)?;

        // Return a copy of the payload
        Ok(Some(self.buffer.clone()))
    }

    /// Get a reference to the underlying reader.
    pub fn reader(&self) -> &R {
        &self.reader
    }

    /// Get a mutable reference to the underlying reader.
    pub fn reader_mut(&mut self) -> &mut R {
        &mut self.reader
    }

    /// Consume this reader and return the underlying reader.
    pub fn into_inner(self) -> R {
        self.reader
    }
}

impl<R: Read> Iterator for StreamReader<R> {
    type Item = Result<Vec<u8>>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.read_message() {
            Ok(Some(payload)) => Some(Ok(payload)),
            Ok(None) => None, // End of stream
            Err(e) => Some(Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::writer::StreamWriter;
    use flatbuffers::FlatBufferBuilder;
    use std::io::Cursor;

    #[test]
    fn test_read_message_with_checksum() {
        // Write a message first
        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), ChecksumType::XxHash64);

        // Create a simple FlatBuffer
        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("test data");
        builder.finish(data, None);
        writer.write_message(&mut builder).unwrap();

        // Now read it back
        let data = buffer;
        let mut reader = StreamReader::new(Cursor::new(data), ChecksumType::XxHash64);

        let result = reader.read_message().unwrap();
        assert!(result.is_some());

        let payload = result.unwrap();
        assert!(!payload.is_empty());
    }

    #[test]
    fn test_read_message_without_checksum() {
        // Write a message first
        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), ChecksumType::None);

        // Create a simple FlatBuffer
        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("no checksum");
        builder.finish(data, None);
        writer.write_message(&mut builder).unwrap();

        // Now read it back
        let data = buffer;
        let mut reader = StreamReader::new(Cursor::new(data), ChecksumType::None);

        let result = reader.read_message().unwrap();
        assert!(result.is_some());

        let payload = result.unwrap();
        assert!(!payload.is_empty());
    }

    #[test]
    fn test_read_multiple_messages() {
        // Write multiple messages
        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), ChecksumType::XxHash64);

        for i in 0..3 {
            let mut builder = FlatBufferBuilder::new();
            let data = builder.create_string(&format!("message {}", i));
            builder.finish(data, None);
            writer.write_message(&mut builder).unwrap();
        }

        // Read them back
        let data = buffer;
        let mut reader = StreamReader::new(Cursor::new(data), ChecksumType::XxHash64);

        let mut count = 0;
        while let Some(result) = reader.next() {
            assert!(result.is_ok());
            count += 1;
        }

        assert_eq!(count, 3);
    }

    #[test]
    fn test_read_empty_stream() {
        let empty_data = Vec::new();
        let mut reader = StreamReader::new(Cursor::new(empty_data), ChecksumType::XxHash64);

        let result = reader.read_message().unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_checksum_mismatch() {
        // Write a message with checksum
        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), ChecksumType::XxHash64);

        // Create a simple FlatBuffer
        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("test data");
        builder.finish(data, None);
        writer.write_message(&mut builder).unwrap();

        // Corrupt the data by flipping a bit
        let mut data = buffer;
        if data.len() > 20 {
            data[20] ^= 1; // Flip a bit in the payload
        }

        // Try to read the corrupted data
        let mut reader = StreamReader::new(Cursor::new(data), ChecksumType::XxHash64);
        let result = reader.read_message();

        // Should get a checksum mismatch error
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::ChecksumMismatch { .. } => {} // Expected
            e => panic!("Expected ChecksumMismatch error, got: {:?}", e),
        }
    }
}
