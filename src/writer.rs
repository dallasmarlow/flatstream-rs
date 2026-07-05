//! A generic, composable writer for `flatstream`.

use crate::error::Result;
use crate::framing::Framer;
use crate::policy::{MemoryPolicy, ReclamationInfo};
use crate::traits::StreamSerialize;
use flatbuffers::{DefaultAllocator, FlatBufferBuilder};
use std::io::Write;

/// Installed-policy state: the policy, its baseline (cached from
/// `MemoryPolicy::baseline_capacity()` at installation so the steady-state gate
/// is a plain integer compare), and the means to rebuild the internal builder.
///
/// The factory closure exists because a reclaim must construct a *fresh*
/// `FlatBufferBuilder`, and only the caller knows how to do that for a custom
/// allocator. For the default allocator it is simply `FlatBufferBuilder::with_capacity`.
struct PolicySlot<'a, A: flatbuffers::Allocator> {
    policy: Box<dyn MemoryPolicy>,
    baseline_capacity: usize,
    make_builder: Box<dyn FnMut(usize) -> FlatBufferBuilder<'a, A> + Send + 'a>,
}

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
///    - Single builder can cause memory bloat with mixed sizes (see below)
/// 2. **Expert mode**: User manages builder externally
///    - Use `write_finished()` method
///    - Enables multiple builders for different message types
///    - Better memory control for mixed workloads
///
/// ## Memory Reclamation (simple mode)
///
/// The internal builder grows to the largest message seen and keeps that
/// capacity. For long-running processes with bursty workloads, an optional
/// [`MemoryPolicy`] can be installed with [`with_memory_policy`](Self::with_memory_policy)
/// to shrink the builder back to the policy's baseline capacity once it is
/// over-provisioned. The baseline is policy configuration
/// (`MemoryPolicy::baseline_capacity`, default 16 KiB):
///
/// ```ignore
/// let mut writer = StreamWriter::new(file, DefaultFramer)
///     .with_memory_policy(AdaptiveWatermarkPolicy::new(4, 5).with_baseline(16 * 1024));
/// ```
///
/// The policy is consulted once per `write()` — a single predictable branch
/// when no policy is installed. **Policies apply to simple mode only**: in
/// expert mode (`write_finished()`) the caller owns the builder, so the writer
/// cannot and does not reclaim it.
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
/// To combine a custom allocator with a memory policy, use
/// [`with_memory_policy_and_factory`](Self::with_memory_policy_and_factory) and
/// supply the closure that rebuilds your builder on reclaim.
pub struct StreamWriter<'a, W: Write, F: Framer, A = DefaultAllocator>
where
    A: flatbuffers::Allocator,
{
    writer: W,
    framer: F,
    builder: FlatBufferBuilder<'a, A>,
    policy: Option<PolicySlot<'a, A>>,
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
            policy: None,
        }
    }

    /// Creates a new `StreamWriter` with a pre-constructed builder.
    /// Useful for pre-sizing.
    pub fn with_builder(writer: W, framer: F, builder: FlatBufferBuilder<'a>) -> Self {
        Self {
            writer,
            framer,
            builder,
            policy: None,
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
            policy: None,
        }
    }

    /// Installs a memory reclamation policy on this writer (simple mode only).
    ///
    /// After each successful `write()`, the policy observes the message size and
    /// current builder capacity; when it fires, the internal builder is replaced
    /// with a fresh one at the policy's baseline capacity
    /// (`MemoryPolicy::baseline_capacity`, cached here at installation). The
    /// policy is consulted only while the builder's capacity exceeds that
    /// baseline — at or below it there is nothing to reclaim.
    ///
    /// Has no effect on `write_finished()`, where the caller owns the builder.
    pub fn with_memory_policy<P: MemoryPolicy + 'static>(mut self, policy: P) -> Self {
        self.policy = Some(PolicySlot {
            baseline_capacity: policy.baseline_capacity(),
            policy: Box::new(policy),
            make_builder: Box::new(FlatBufferBuilder::with_capacity),
        });
        self
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
    /// let writer = StreamWriter::with_builder_alloc(file, framer, builder);
    /// ```
    pub fn with_builder_alloc(writer: W, framer: F, builder: FlatBufferBuilder<'a, A>) -> Self {
        Self {
            writer,
            framer,
            builder,
            policy: None,
        }
    }

    /// Installs a memory reclamation policy together with a builder factory.
    ///
    /// This is the custom-allocator variant of
    /// [`with_memory_policy`](Self::with_memory_policy): a reclaim replaces the
    /// internal builder with `make_builder(policy.baseline_capacity())`, so the
    /// factory decides how a fresh builder (and its allocator) is constructed.
    pub fn with_memory_policy_and_factory<P, M>(mut self, policy: P, make_builder: M) -> Self
    where
        P: MemoryPolicy + 'static,
        M: FnMut(usize) -> FlatBufferBuilder<'a, A> + Send + 'a,
    {
        self.policy = Some(PolicySlot {
            baseline_capacity: policy.baseline_capacity(),
            policy: Box::new(policy),
            make_builder: Box::new(make_builder),
        });
        self
    }

    /// Writes a serializable item to the stream using the internally managed builder.
    /// The builder is reset before serialization.
    ///
    /// This is the **simple mode** API - convenient for uniform message sizes.
    ///
    /// # Pitfalls
    /// - The internal builder can grow to the largest message and stay that size; for
    ///   mixed sizes, install a [`MemoryPolicy`] or use expert mode with multiple
    ///   builders to avoid bloat.
    /// - Excellent for uniform, small-to-medium messages.
    ///
    /// # Example
    /// ```ignore
    /// writer.write(&"Hello, world!")?;
    /// writer.write(&my_telemetry_event)?;
    /// ```
    #[inline]
    pub fn write<T: StreamSerialize>(&mut self, item: &T) -> Result<()> {
        // Reset the internal builder for reuse
        self.builder.reset();

        // Direct serialization to the builder - no temporary allocations or copying
        item.serialize(&mut self.builder)?;

        // Get the finished payload from the builder
        let payload = self.builder.finished_data();
        let last_message_size = payload.len();

        // Delegate framing and writing to the strategy
        self.framer.frame_and_write(&mut self.writer, payload)?;

        // Evaluate the policy only after a successful write, so the payload we
        // just framed is never invalidated. One predictable branch when no
        // policy is installed; the machinery is outlined to keep this hot path
        // small.
        if self.policy.is_some() {
            self.evaluate_memory_policy(last_message_size);
        }

        Ok(())
    }

    /// Consults the installed policy after a successful `write()`. Outlined
    /// (`inline(never)`) to keep `write()`'s inlinable body minimal for
    /// writers without a policy.
    #[inline(never)]
    fn evaluate_memory_policy(&mut self, last_message_size: usize) {
        let Some(slot) = self.policy.as_mut() else {
            return;
        };
        // Capacity read: `FlatBufferBuilder` exposes no capacity() getter.
        // mut_finished_buffer() returns (&mut backing_buffer, start_index);
        // the slice length is the backing buffer size — our effective
        // capacity. O(1), no allocation, and safe here because the builder
        // is finished and the frame has been written.
        let (buf, _start_idx) = self.builder.mut_finished_buffer();
        let current_capacity = buf.len();

        // At or below the policy's baseline there is nothing to reclaim —
        // skip the policy entirely so its hysteresis state cannot churn
        // (rebuilding a baseline-sized builder into an identical one would
        // be pure allocator noise).
        if current_capacity > slot.baseline_capacity {
            if let Some(reason) = slot
                .policy
                .should_reset(last_message_size, current_capacity)
            {
                // Drop the over-provisioned builder and rebuild at the
                // baseline capacity — resets the stream's high-water mark.
                self.builder = (slot.make_builder)(slot.baseline_capacity);
                slot.policy.on_reclaim(&ReclamationInfo {
                    reason,
                    last_message_size,
                    capacity_before: current_capacity,
                    capacity_after: slot.baseline_capacity,
                });
            }
        }
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
    /// # Memory policy
    /// Any installed [`MemoryPolicy`] does **not** apply here: the builder is
    /// owned by the caller, so reclaiming it is the caller's responsibility
    /// (drop and recreate the builder, or use multiple right-sized builders).
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

    #[test]
    fn test_write_with_policy_installed() {
        use crate::policy::NoOpPolicy;

        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer)
            .with_memory_policy(NoOpPolicy);

        assert!(writer.write(&"policy message 1").is_ok());
        assert!(writer.write(&"policy message 2").is_ok());

        let data = buffer;
        assert!(!data.is_empty());
    }

    #[test]
    fn test_writer_with_policy_is_send() {
        use crate::policy::NoOpPolicy;

        fn assert_send<T: Send>(_: &T) {}
        let writer =
            StreamWriter::new(std::io::sink(), DefaultFramer).with_memory_policy(NoOpPolicy);
        assert_send(&writer);
    }
}
