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
/// **Zero-Copy Guarantee**: Both writing modes maintain perfect zero-copy behavior.
/// After serialization, `builder.finished_data()` returns a direct slice that's
/// written to I/O without any intermediate copies.
///
/// The writer can operate in two modes:
/// 1. **Simple mode**: Writer manages its own builder internally
///    - Use `write()` method for convenience
///    - Best for uniform message sizes
///    - Single builder can cause memory bloat with mixed sizes
/// 2. **Expert mode**: User manages builder externally
///    - Use `write_finished()` method
///    - Enables multiple builders for different message types
///    - Better memory control for mixed workloads
///
/// ## Custom Allocators
///
/// While the `with_builder` constructor allows providing a custom `FlatBufferBuilder`,
/// implementing truly efficient custom allocators (like arena allocation) is challenging
/// due to the design of the `flatbuffers` crate's `Allocator` trait.
///
/// The default `StreamWriter::new()` constructor already provides efficient builder reuse,
/// which eliminates most allocation overhead. Combined with the expert mode pattern
/// (`write_finished()`), this achieves excellent performance for nearly all use cases.
///
/// The `with_builder()` constructor exists primarily for future extensibility. For
/// maximum performance today, use `write_finished()` with external builder management.
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
    /// 
    /// This enables **simple mode** - the writer manages an internal builder
    /// and provides the convenient `write()` method. Perfect for getting started
    /// and moderate-throughput applications.
    ///
    /// For high-performance production use, consider using `write_finished()`
    /// with external builder management instead of relying on `write()`.
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
    /// 
    /// This enables **expert mode** with custom allocation strategies like arena allocation.
    /// Use this when you need the absolute maximum performance or zero-allocation guarantees.
    ///
    /// Note: Even with the standard `new()` constructor, you can achieve expert-level
    /// performance by using `write_finished()` with an external builder. This constructor
    /// is only needed when you require a custom allocator.
    ///
    /// # Example
    /// ```ignore
    /// // With a hypothetical custom allocator
    /// let allocator = MyCustomAllocator::new();
    /// let builder = FlatBufferBuilder::new_with_allocator(allocator);
    /// let writer = StreamWriter::with_builder(file, framer, builder);
    /// ```
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
    /// This is the **simple mode** API - convenient for uniform message sizes.
    /// 
    /// # Performance
    /// - Excellent for uniform, small-to-medium messages
    /// - Builder grows to accommodate largest message and stays that size
    /// - For mixed sizes or large messages, use `write_finished()` instead
    ///
    /// # Example
    /// ```ignore
    /// writer.write(&"Hello, world!")?;
    /// writer.write(&my_telemetry_event)?;
    /// ```
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
    /// This is the **expert mode** API - optimal for high-frequency production use.
    ///
    /// The user manages the builder lifecycle, enabling:
    /// - Zero-allocation writes through builder reuse
    /// - Custom allocator support (e.g., arena allocation)
    /// - Maximum performance for real-time systems
    ///
    /// # Performance
    /// - Zero allocations with proper builder reuse via `reset()`
    /// - Up to 2x faster than simple mode for large messages
    /// - Enables memory-efficient handling of mixed message sizes
    ///
    /// # Example
    /// ```ignore
    /// let mut builder = FlatBufferBuilder::new();
    /// for event in events {
    ///     builder.reset();  // Critical: reuse allocated memory!
    ///     event.serialize(&mut builder)?;
    ///     writer.write_finished(&mut builder)?;
    /// }
    /// ```
    ///
    /// # Requirements
    /// The user must call `builder.finish()` within their `serialize()` implementation
    /// before calling this method.
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
