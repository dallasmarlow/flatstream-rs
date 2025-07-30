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
/// The writer can operate in two modes:
/// 1. **Simple mode**: Writer manages its own builder internally (default allocator)
/// 2. **Expert mode**: User provides a custom `FlatBufferBuilder` (e.g., with arena allocation)
pub struct StreamWriter<'a, W: Write, F: Framer, A = flatbuffers::DefaultAllocator>
where
    A: flatbuffers::Allocator,
{
    writer: W,
    framer: F,
    builder: FlatBufferBuilder<'a, A>,
}

impl<'a, W: Write, F: Framer> StreamWriter<'a, W, F> {
    /// Creates a new `StreamWriter` with a default `FlatBufferBuilder`.
    /// This is the simple mode for most use cases.
    pub fn new(writer: W, framer: F) -> Self {
        Self {
            writer,
            framer,
            builder: FlatBufferBuilder::new(),
        }
    }
}

impl<'a, W: Write, F: Framer, A> StreamWriter<'a, W, F, A>
where
    A: flatbuffers::Allocator,
{
    /// Creates a new `StreamWriter` with a user-provided `FlatBufferBuilder`.
    /// This is the expert mode for custom allocation strategies like arena allocation.
    pub fn with_builder(writer: W, framer: F, builder: FlatBufferBuilder<'a, A>) -> Self {
        Self {
            writer,
            framer,
            builder,
        }
    }

    /// Writes a serializable item to the stream using the internally managed builder.
    /// The builder is reset before serialization.
    ///
    /// This method maintains zero-copy performance by directly using the builder
    /// without any temporary allocations or data copying.
    pub fn write<T: StreamSerialize>(&mut self, item: &T) -> Result<()> {
        // Reset the internal builder for reuse
        self.builder.reset();

        // Direct serialization to the builder - no temporary allocations or copying
        item.serialize(&mut self.builder)?;

        // Get the finished payload from the builder
        let payload = self.builder.finished_data();

        // Delegate framing and writing to the strategy
        self.framer.frame_and_write(&mut self.writer, payload)
    }

    /// Writes a finished FlatBuffer message to the stream.
    /// This is the expert mode where the user manages the builder lifecycle.
    ///
    /// The user is responsible for calling `builder.finish()` before this method.
    /// This method will access the finished data and frame it according to the framer strategy.
    pub fn write_finished(&mut self, builder: &mut FlatBufferBuilder) -> Result<()> {
        // Get the finished payload from the builder
        let payload = builder.finished_data();

        // Delegate framing and writing to the strategy
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

        // Create and finish a builder
        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("test data");
        builder.finish(data, None);

        assert!(writer.write_finished(&mut builder).is_ok());

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

        // Create and finish a builder
        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("test data");
        builder.finish(data, None);

        assert!(writer.write_finished(&mut builder).is_ok());

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

        // Create and finish a builder
        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("no checksum");
        builder.finish(data, None);

        assert!(writer.write_finished(&mut builder).is_ok());

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
            let mut builder = FlatBufferBuilder::new();
            let data = builder.create_string(&format!("message {}", i));
            builder.finish(data, None);
            assert!(writer.write_finished(&mut builder).is_ok());
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
            let mut builder = FlatBufferBuilder::new();
            let data = builder.create_string(&format!("message {}", i));
            builder.finish(data, None);
            assert!(writer.write_finished(&mut builder).is_ok());
        }

        let data = buffer;
        assert!(!data.is_empty());
    }

    #[test]
    fn test_simple_write_mode() {
        let mut buffer = Vec::new();
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        // Test the simple write mode with a string
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

        // Test multiple simple writes
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
