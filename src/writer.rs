//! A generic, composable writer for `flatstream`.

use crate::error::Result;
use crate::framing::Framer;
use crate::policy::{MemoryPolicy, NoOpPolicy, ReclamationReason};
use crate::traits::StreamSerialize;
use flatbuffers::{DefaultAllocator, FlatBufferBuilder};
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
pub struct StreamWriter<'a, W: Write, F: Framer, P = NoOpPolicy, A = DefaultAllocator>
where
    P: MemoryPolicy,
    A: flatbuffers::Allocator,
{
    writer: W,
    framer: F,
    builder: FlatBufferBuilder<'a, A>,
    policy: P,
    default_buffer_capacity: usize,
    on_reclaim: Option<Box<ReclaimCallback>>,
}

/// Information passed to the optional reclamation callback when a reset occurs.
pub struct ReclamationInfo {
    pub reason: ReclamationReason,
    pub last_message_size: usize,
    pub capacity_before: usize,
    pub capacity_after: usize,
}

type ReclaimCallback = dyn Fn(&ReclamationInfo) + Send + 'static;

const DEFAULT_BUILDER_CAPACITY: usize = 16 * 1024;

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
            policy: NoOpPolicy,
            default_buffer_capacity: DEFAULT_BUILDER_CAPACITY,
            on_reclaim: None,
        }
    }

    /// Creates a new `StreamWriter` with a pre-constructed builder.
    /// Useful for pre-sizing.
    pub fn with_builder(writer: W, framer: F, builder: FlatBufferBuilder<'a>) -> Self {
        Self {
            writer,
            framer,
            builder,
            policy: NoOpPolicy,
            default_buffer_capacity: DEFAULT_BUILDER_CAPACITY,
            on_reclaim: None,
        }
    }

    /// Creates a new `StreamWriter` with an internal builder pre-allocated to `capacity` bytes.
    /// Mirrors `StreamReader::with_capacity` for API symmetry.
    /// Useful when you know typical payload sizes and want to avoid early growth.
    pub fn with_capacity(writer: W, framer: F, capacity: usize) -> Self {
        Self {
            writer,
            framer,
            builder: FlatBufferBuilder::with_capacity(capacity),
            policy: NoOpPolicy,
            default_buffer_capacity: capacity,
            on_reclaim: None,
        }
    }

    /// Starts a fluent builder for configuring an optional memory policy.
    pub fn builder(writer: W, framer: F) -> StreamWriterBuilder<'a, W, F, NoOpPolicy> {
        StreamWriterBuilder {
            writer,
            framer,
            policy: NoOpPolicy,
            default_buffer_capacity: DEFAULT_BUILDER_CAPACITY,
            on_reclaim: None,
            _phantom: core::marker::PhantomData,
        }
    }
}

impl<'a, W: Write, F: Framer, P, A> StreamWriter<'a, W, F, P, A>
where
    P: MemoryPolicy,
    A: flatbuffers::Allocator,
{
    // write() is only available when using the default allocator internally.

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
    /// before calling this method. This method assumes the builder contains a finished root.
    pub fn write_finished<A2: flatbuffers::Allocator>(
        &mut self,
        builder: &mut FlatBufferBuilder<A2>,
    ) -> Result<()> {
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

    /// Returns a reference to the underlying writer.
    pub fn get_ref(&self) -> &W {
        &self.writer
    }

    /// Returns a mutable reference to the underlying writer.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.writer
    }

    /// Returns a reference to the framer strategy.
    pub fn framer(&self) -> &F {
        &self.framer
    }
}

impl<'a, W: Write, F: Framer, A> StreamWriter<'a, W, F, NoOpPolicy, A>
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
    /// let writer = StreamWriter::with_builder_alloc(file, framer, builder);
    /// ```
    pub fn with_builder_alloc(writer: W, framer: F, builder: FlatBufferBuilder<'a, A>) -> Self {
        Self {
            writer,
            framer,
            builder,
            policy: NoOpPolicy,
            default_buffer_capacity: DEFAULT_BUILDER_CAPACITY,
            on_reclaim: None,
        }
    }
}

impl<'a, W: Write, F: Framer, P> StreamWriter<'a, W, F, P, DefaultAllocator>
where
    P: MemoryPolicy,
{
    /// Writes a serializable item to the stream using the internally managed builder.
    /// The builder is reset before serialization.
    ///
    /// This is the **simple mode** API - convenient for uniform message sizes.
    #[inline]
    pub fn write<T: StreamSerialize>(&mut self, item: &T) -> Result<()> {
        // Reset the internal builder for reuse
        self.builder.reset();

        // Direct serialization to the builder - no temporary allocations or copying
        item.serialize(&mut self.builder)?;

        // Get the finished payload from the builder
        let payload = self.builder.finished_data();

        // Delegate framing and writing to the strategy
        self.framer.frame_and_write(&mut self.writer, payload)?;

        // Evaluate policy after a successful write
        let last_message_size = payload.len();
        // Capacity read:
        // - The FlatBufferBuilder API does not expose a direct capacity() getter.
        // - mut_finished_buffer() returns (&mut [u8], start_index) for the finished buffer.
        // - The slice length corresponds to the backing buffer size (our effective "capacity").
        // - This is O(1), no allocations/copies; we do not mutate the slice.
        // - Safe here because the buffer is finished and framing/write have completed.
        let (buf, _start_idx) = self.builder.mut_finished_buffer();
        let current_capacity = buf.len();
        if let Some(reason) = self
            .policy
            .should_reset(last_message_size, current_capacity)
        {
            // Recreate the builder with a configured default capacity
            self.builder = FlatBufferBuilder::with_capacity(self.default_buffer_capacity);
            if let Some(cb) = &self.on_reclaim {
                (cb)(&ReclamationInfo {
                    reason,
                    last_message_size,
                    capacity_before: current_capacity,
                    capacity_after: self.default_buffer_capacity,
                });
            }
        }

        Ok(())
    }
}

/// Fluent builder for `StreamWriter` configuration (default allocator only).
pub struct StreamWriterBuilder<'a, W, F, P = NoOpPolicy>
where
    W: Write,
    F: Framer,
    P: MemoryPolicy,
{
    writer: W,
    framer: F,
    policy: P,
    default_buffer_capacity: usize,
    on_reclaim: Option<Box<ReclaimCallback>>,
    _phantom: core::marker::PhantomData<&'a ()>,
}

impl<'a, W, F, P> StreamWriterBuilder<'a, W, F, P>
where
    W: Write,
    F: Framer,
    P: MemoryPolicy + 'a,
{
    pub fn with_policy<P2: MemoryPolicy>(self, policy: P2) -> StreamWriterBuilder<'a, W, F, P2> {
        StreamWriterBuilder {
            writer: self.writer,
            framer: self.framer,
            policy,
            default_buffer_capacity: self.default_buffer_capacity,
            on_reclaim: self.on_reclaim,
            _phantom: core::marker::PhantomData,
        }
    }

    pub fn with_default_capacity(mut self, capacity: usize) -> Self {
        self.default_buffer_capacity = capacity;
        self
    }

    pub fn with_reclaim_callback<Cb>(mut self, callback: Cb) -> Self
    where
        Cb: Fn(&ReclamationInfo) + Send + 'static,
    {
        self.on_reclaim = Some(Box::new(callback));
        self
    }

    pub fn build(self) -> StreamWriter<'a, W, F, P, DefaultAllocator> {
        StreamWriter {
            writer: self.writer,
            framer: self.framer,
            builder: FlatBufferBuilder::with_capacity(self.default_buffer_capacity),
            policy: self.policy,
            default_buffer_capacity: self.default_buffer_capacity,
            on_reclaim: self.on_reclaim,
        }
    }

    pub fn build_dyn(self) -> StreamWriter<'a, W, F, Box<dyn MemoryPolicy + 'a>, DefaultAllocator> {
        StreamWriter {
            writer: self.writer,
            framer: self.framer,
            builder: FlatBufferBuilder::with_capacity(self.default_buffer_capacity),
            policy: Box::new(self.policy),
            default_buffer_capacity: self.default_buffer_capacity,
            on_reclaim: self.on_reclaim,
        }
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
    fn test_write_default_framer() {
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
        // DefaultFramer: 4 bytes (length) + payload (no checksum)
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
            let data = builder.create_string(&format!("message {i}"));
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
            let data = builder.create_string(&format!("message {i}"));
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
