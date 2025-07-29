use crate::checksum::ChecksumType;
use crate::error::Result;
use flatbuffers::FlatBufferBuilder;
use std::io::Write;

/// A writer for streaming FlatBuffers messages with optional checksumming.
///
/// The `StreamWriter` writes messages in the format:
/// `[4-byte Payload Length (u32, little-endian) | 8-byte Checksum (u64, little-endian, if enabled) | FlatBuffer Payload]`
pub struct StreamWriter<W: Write> {
    writer: W,
    checksum_type: ChecksumType,
}

impl<W: Write> StreamWriter<W> {
    /// Create a new `StreamWriter` with the specified underlying writer and checksum type.
    ///
    /// # Arguments
    /// * `writer` - The underlying writer to write to
    /// * `checksum_type` - The type of checksum to use for data integrity
    pub fn new(writer: W, checksum_type: ChecksumType) -> Self {
        Self {
            writer,
            checksum_type,
        }
    }

    /// Write a FlatBuffers message to the stream.
    ///
    /// This method:
    /// 1. Finishes the FlatBuffer builder to get the serialized payload
    /// 2. Calculates the checksum of the payload
    /// 3. Writes the payload length (4 bytes, little-endian)
    /// 4. Writes the checksum (8 bytes, little-endian, if enabled)
    /// 5. Writes the payload bytes
    /// 6. Resets the builder for reuse
    ///
    /// # Arguments
    /// * `builder` - The FlatBuffer builder containing the message
    /// * `root` - The root offset of the FlatBuffer message
    ///
    /// # Returns
    /// `Ok(())` on success, or an error if writing fails
    pub fn write_message<'a, 'b>(&mut self, builder: &'a mut FlatBufferBuilder<'b>) -> Result<()> {
        // The builder should already be finished
        let payload = builder.finished_data();

        // Calculate checksum
        let checksum = self.checksum_type.calculate_checksum(payload);

        // Write payload length (4 bytes, little-endian)
        let payload_length = payload.len() as u32;
        self.writer.write_all(&payload_length.to_le_bytes())?;

        // Write checksum (8 bytes, little-endian, if enabled)
        if self.checksum_type != ChecksumType::None {
            self.writer.write_all(&checksum.to_le_bytes())?;
        }

        // Write the payload
        self.writer.write_all(payload)?;

        // Reset the builder for reuse
        builder.reset();

        Ok(())
    }

    /// Flush any buffered data to the underlying writer.
    ///
    /// # Returns
    /// `Ok(())` on success, or an error if flushing fails
    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }

    /// Get a reference to the underlying writer.
    pub fn writer(&self) -> &W {
        &self.writer
    }

    /// Get a mutable reference to the underlying writer.
    pub fn writer_mut(&mut self) -> &mut W {
        &mut self.writer
    }

    /// Consume this writer and return the underlying writer.
    pub fn into_inner(self) -> W {
        self.writer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_write_message_with_checksum() {
        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), ChecksumType::XxHash64);

        // Create a simple FlatBuffer with just a string
        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("test data");
        builder.finish(data, None);
        assert!(writer.write_message(&mut builder).is_ok());

        // Verify the written data structure
        let data = buffer;
        assert!(!data.is_empty());

        // Should have: 4 bytes (length) + 8 bytes (checksum) + payload
        assert!(data.len() >= 12);
    }

    #[test]
    fn test_write_message_without_checksum() {
        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), ChecksumType::None);

        // Create a simple FlatBuffer with just a string
        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("no checksum");
        builder.finish(data, None);
        assert!(writer.write_message(&mut builder).is_ok());

        // Verify the written data structure
        let data = buffer;
        assert!(!data.is_empty());

        // Should have: 4 bytes (length) + payload (no checksum)
        assert!(data.len() >= 4);
    }

    #[test]
    fn test_multiple_messages() {
        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), ChecksumType::XxHash64);

        for i in 0..3 {
            let mut builder = FlatBufferBuilder::new();
            let data = builder.create_string(&format!("message {}", i));
            builder.finish(data, None);
            assert!(writer.write_message(&mut builder).is_ok());
        }

        let data = buffer;
        assert!(!data.is_empty());
    }

    #[test]
    fn test_flush() {
        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), ChecksumType::None);

        assert!(writer.flush().is_ok());
    }
}
