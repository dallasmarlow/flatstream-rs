//! A generic, composable writer for `flatstream`.

use crate::error::Result;
use crate::framing::Framer;
use crate::traits::StreamSerialize;
use flatbuffers::FlatBufferBuilder;
use std::io::Write;

/// A writer for streaming FlatBuffer messages.
///
/// This writer is generic over a `Framer` strategy, which defines how
/// each message is framed in the byte stream (e.g., with or without a checksum).
///
/// The writer provides three levels of API:
/// 1. `write()` - Simple API that allocates a builder internally
/// 2. `write_with_builder()` - Zero-allocation API with external builder
/// 3. `write_finished()` - Lowest-level API for pre-built buffers
pub struct StreamWriter<W: Write, F: Framer> {
    writer: W,
    framer: F,
}

impl<W: Write, F: Framer> StreamWriter<W, F> {
    /// Creates a new `StreamWriter`.
    pub fn new(writer: W, framer: F) -> Self {
        Self { writer, framer }
    }

    /// Writes a serializable item to the stream.
    /// This is a convenience method that allocates a builder internally.
    /// For zero-allocation writes, use `write_with_builder`.
    pub fn write<T: StreamSerialize>(&mut self, item: &T) -> Result<()> {
        let mut builder = FlatBufferBuilder::new();
        item.serialize_to(&mut builder)?;
        let payload = builder.finished_data();
        self.framer.frame_and_write(&mut self.writer, payload)
    }

    /// Writes a serializable item to the stream using an external builder.
    /// This is the zero-allocation path for high-performance use cases.
    /// The builder will be reset and reused for serialization.
    pub fn write_with_builder<T: StreamSerialize>(
        &mut self,
        builder: &mut FlatBufferBuilder,
        item: &T,
    ) -> Result<()> {
        item.serialize_to(builder)?;
        let payload = builder.finished_data();
        self.framer.frame_and_write(&mut self.writer, payload)
    }

    /// Writes a finished FlatBuffer message to the stream.
    /// This is the lowest-level API for users who need complete control.
    /// The user is responsible for building and finishing the buffer.
    pub fn write_finished(&mut self, builder: &mut FlatBufferBuilder) -> Result<()> {
        let payload = builder.finished_data();
        self.framer.frame_and_write(&mut self.writer, payload)
    }

    /// Flushes the underlying writer.
    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }

    /// Consumes the writer, returning the underlying writer.
    pub fn into_inner(self) -> W {
        self.writer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::DefaultFramer;

    #[cfg(feature = "xxhash")]
    use crate::{ChecksumFramer, XxHash64};
    use std::io::Cursor;

    #[test]
    fn test_write_with_checksum() {
        let mut buffer = Vec::new();
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        // Test the convenience write method
        assert!(writer.write(&"test data").is_ok());

        let data = buffer;
        assert!(!data.is_empty());
        // Should have: 4 bytes (length) + payload (no checksum)
        assert!(data.len() >= 4);
    }

    #[cfg(feature = "xxhash")]
    #[test]
    fn test_write_with_checksum_feature() {
        let mut buffer = Vec::new();
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        // Test the convenience write method
        assert!(writer.write(&"test data").is_ok());

        let data = buffer;
        assert!(!data.is_empty());
        // Should have: 4 bytes (length) + 8 bytes (checksum) + payload
        assert!(data.len() >= 12);
    }

    #[test]
    fn test_write_without_checksum() {
        let mut buffer = Vec::new();
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        // Test the convenience write method
        assert!(writer.write(&"no checksum").is_ok());

        let data = buffer;
        assert!(!data.is_empty());
        // Should have: 4 bytes (length) + payload (no checksum)
        assert!(data.len() >= 4);
    }

    #[test]
    fn test_multiple_messages() {
        let mut buffer = Vec::new();
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        for i in 0..3 {
            assert!(writer.write(&format!("message {}", i)).is_ok());
        }

        let data = buffer;
        assert!(!data.is_empty());
    }

    #[cfg(feature = "xxhash")]
    #[test]
    fn test_multiple_messages_with_checksum() {
        let mut buffer = Vec::new();
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        for i in 0..3 {
            assert!(writer.write(&format!("message {}", i)).is_ok());
        }

        let data = buffer;
        assert!(!data.is_empty());
    }

    #[test]
    fn test_simple_write_mode() {
        let mut buffer = Vec::new();
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        // Test the convenience write method
        assert!(writer.write(&"test message").is_ok());

        let data = buffer;
        assert!(!data.is_empty());
        // Should have: 4 bytes (length) + payload
        assert!(data.len() >= 4);
    }

    #[test]
    fn test_multiple_simple_writes() {
        let mut buffer = Vec::new();
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        // Test multiple writes using the convenience method
        assert!(writer.write(&"message 1").is_ok());
        assert!(writer.write(&"message 2").is_ok());
        assert!(writer.write(&"message 3").is_ok());

        let data = buffer;
        assert!(!data.is_empty());
    }

    #[test]
    fn test_flush() {
        let mut buffer = Vec::new();
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        assert!(writer.flush().is_ok());
    }
}
