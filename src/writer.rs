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

/// Wraps the underlying writer and counts bytes handed to it, so a
/// `StreamWriter` can report the offset and on-wire length of each frame (see
/// [`FrameReceipt`]) without the `Framer` trait having to report anything. The
/// count reflects bytes *actually accepted* by `W`, so it stays correct for any
/// framer, custom ones included.
struct CountingWriter<W> {
    inner: W,
    count: u64,
}

impl<W> CountingWriter<W> {
    #[inline]
    fn new(inner: W) -> Self {
        Self { inner, count: 0 }
    }

    #[inline]
    fn get_ref(&self) -> &W {
        &self.inner
    }

    #[inline]
    fn get_mut(&mut self) -> &mut W {
        &mut self.inner
    }

    #[inline]
    fn into_inner(self) -> W {
        self.inner
    }
}

impl<W: Write> Write for CountingWriter<W> {
    #[inline]
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.count += n as u64;
        Ok(n)
    }

    #[inline]
    fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
        // Delegate to the inner writer's own (possibly optimized) `write_all`
        // and count only on success. A mid-frame error tears the frame and the
        // stream is recovered/truncated per the recovery contract, so a partial
        // count we cannot observe here does not affect a well-formed stream.
        self.inner.write_all(buf)?;
        self.count += buf.len() as u64;
        Ok(())
    }

    #[inline]
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// A writer for streaming FlatBuffer messages.
///
/// This writer is generic over a `Framer` strategy, which defines how
/// each message is framed in the byte stream (e.g., with or without a checksum).
///
/// **Copy behavior**: Both writing modes pass `builder.finished_data()` to the
/// `Write` target directly — the library introduces no intermediate payload
/// copy. (The target itself may copy, e.g. `BufWriter` staging into its
/// buffer; that is the target's contract, not this crate's.)
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
    writer: CountingWriter<W>,
    framer: F,
    builder: FlatBufferBuilder<'a, A>,
    policy: Option<PolicySlot<'a, A>>,
    /// Offset that [`FrameReceipt`] offsets and [`StreamWriter::bytes_written`]
    /// are measured from. 0 unless set via [`StreamWriter::with_start_offset`].
    start_offset: u64,
}

/// A [`StreamWriter`] fixed to the default allocator and a `'static` builder
/// lifetime.
///
/// `StreamWriter`'s `'a` lifetime comes from its internal `FlatBufferBuilder<'a>`
/// and only bites when the builder borrows external data. Consumers that only
/// call [`write_finished`](StreamWriter::write_finished) (or otherwise never let
/// the builder borrow) never exercise that lifetime yet still have to name or
/// infer it. This alias pins it to `'static`, so such writers read as a plain
/// `OwnedStreamWriter<W, F>`.
pub type OwnedStreamWriter<W, F> = StreamWriter<'static, W, F, DefaultAllocator>;

/// The byte position and on-wire size of a single written frame, returned by
/// the `*_with_receipt` write methods.
///
/// `frame_start` is the offset of the frame's first byte; `wire_len` is the
/// total bytes the frame occupies on the wire (length prefix + optional
/// checksum + payload). The next frame begins at `frame_start + wire_len`.
/// Offsets are relative to the writer's start offset (0 by default, or the value
/// given to [`StreamWriter::with_start_offset`] for a writer positioned over a
/// nonzero region of a file), so they can be recorded in an external index and
/// used to seek a reader — no `8 + payload_len` wire arithmetic in caller code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameReceipt {
    /// Offset of the frame's first byte, relative to the writer's start offset.
    pub frame_start: u64,
    /// Total bytes the frame occupies on the wire.
    pub wire_len: u64,
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
            writer: CountingWriter::new(writer),
            framer,
            builder: FlatBufferBuilder::new(),
            policy: None,
            start_offset: 0,
        }
    }

    /// Creates a new `StreamWriter` with a pre-constructed builder.
    /// Useful for pre-sizing.
    pub fn with_builder(writer: W, framer: F, builder: FlatBufferBuilder<'a>) -> Self {
        Self {
            writer: CountingWriter::new(writer),
            framer,
            builder,
            policy: None,
            start_offset: 0,
        }
    }

    /// Creates a new `StreamWriter` with an internal builder pre-allocated to `capacity` bytes.
    /// Mirrors `StreamReader::with_capacity` for API symmetry.
    /// Useful when you know typical payload sizes and want to avoid early growth.
    pub fn with_capacity(writer: W, framer: F, capacity: usize) -> Self {
        Self {
            writer: CountingWriter::new(writer),
            framer,
            builder: FlatBufferBuilder::with_capacity(capacity),
            policy: None,
            start_offset: 0,
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
            writer: CountingWriter::new(writer),
            framer,
            builder,
            policy: None,
            start_offset: 0,
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
        self.write_with_receipt(item).map(|_| ())
    }

    /// Like [`write`](Self::write), but returns a [`FrameReceipt`] with the byte
    /// offset and on-wire length of the frame just written — the primitive for
    /// building an external index (offset → frame) without duplicating the wire
    /// layout in caller code.
    ///
    /// The offset is captured before framing and the length is the count of
    /// bytes actually accepted by the underlying writer, so the receipt is
    /// correct for any framer, including custom ones.
    #[inline]
    pub fn write_with_receipt<T: StreamSerialize>(&mut self, item: &T) -> Result<FrameReceipt> {
        // Reset the internal builder for reuse
        self.builder.reset();

        // Serialize directly into the reusable builder. The implementation of
        // StreamSerialize controls any temporary work it performs.
        item.serialize(&mut self.builder)?;

        // Get the finished payload from the builder
        let payload = self.builder.finished_data();
        let last_message_size = payload.len();

        // Delegate framing and writing to the strategy, bracketing it with the
        // byte counter so the receipt reflects exactly what reached the wire.
        let frame_start = self.start_offset + self.writer.count;
        self.framer.frame_and_write(&mut self.writer, payload)?;
        let wire_len = (self.start_offset + self.writer.count) - frame_start;

        // Evaluate the policy only after a successful write, so the payload we
        // just framed is never invalidated. One predictable branch when no
        // policy is installed; the machinery is outlined to keep this hot path
        // small.
        if self.policy.is_some() {
            self.evaluate_memory_policy(last_message_size);
        }

        Ok(FrameReceipt {
            frame_start,
            wire_len,
        })
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
    /// - Avoids internal-builder bloat and gives the caller full control over
    ///   builder lifecycle and allocation for mixed message sizes
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
        self.write_finished_with_receipt(builder).map(|_| ())
    }

    /// Like [`write_finished`](Self::write_finished), but returns a
    /// [`FrameReceipt`] with the byte offset and on-wire length of the frame
    /// just written. See [`write_with_receipt`](Self::write_with_receipt) for
    /// the external-index use case.
    #[inline]
    pub fn write_finished_with_receipt<A2: flatbuffers::Allocator>(
        &mut self,
        builder: &mut FlatBufferBuilder<A2>,
    ) -> Result<FrameReceipt> {
        // Get the finished payload from the builder
        let payload = builder.finished_data();

        // Delegate framing and writing to the strategy, bracketing it with the
        // byte counter so the receipt reflects exactly what reached the wire.
        let frame_start = self.start_offset + self.writer.count;
        self.framer.frame_and_write(&mut self.writer, payload)?;
        let wire_len = (self.start_offset + self.writer.count) - frame_start;

        Ok(FrameReceipt {
            frame_start,
            wire_len,
        })
    }

    /// Flushes the underlying writer.
    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }

    /// Consumes the writer, returning the underlying writer.
    pub fn into_inner(self) -> W {
        self.writer.into_inner()
    }

    /// Returns a reference to the underlying writer.
    pub fn get_ref(&self) -> &W {
        self.writer.get_ref()
    }

    /// Returns a mutable reference to the underlying writer.
    ///
    /// Bytes written to the underlying writer through this reference bypass the
    /// frame counter, so a subsequent [`FrameReceipt`] offset will not account
    /// for them. Reserved for inspection, not for out-of-band framing.
    pub fn get_mut(&mut self) -> &mut W {
        self.writer.get_mut()
    }

    /// Returns a reference to the framer strategy.
    pub fn framer(&self) -> &F {
        &self.framer
    }

    /// The current stream offset: the start offset plus every byte framed and
    /// written so far. Equivalently, the `frame_start` the next written frame
    /// will receive. See [`FrameReceipt`].
    pub fn bytes_written(&self) -> u64 {
        self.start_offset + self.writer.count
    }

    /// Sets the offset that [`FrameReceipt`] offsets and
    /// [`bytes_written`](Self::bytes_written) are measured from.
    ///
    /// Use it when the underlying writer is positioned over a nonzero region of
    /// a file (e.g. appending to an existing journal) and you want receipts to
    /// carry absolute file offsets. Defaults to 0; set it before writing.
    #[must_use]
    pub fn with_start_offset(mut self, offset: u64) -> Self {
        self.start_offset = offset;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::DefaultFramer;
    use crate::policy::NoOpPolicy;

    #[cfg(feature = "xxhash")]
    use crate::checksum::Checksum;
    #[cfg(feature = "xxhash")]
    use crate::{ChecksumFramer, XxHash64};
    use std::io::Cursor;

    /// Serializes `s` as a string root into `builder` and returns the exact
    /// payload bytes the writer must put on the wire.
    fn finished(builder: &mut FlatBufferBuilder, s: &str) -> Vec<u8> {
        builder.reset();
        let data = builder.create_string(s);
        builder.finish(data, None);
        builder.finished_data().to_vec()
    }

    #[test]
    fn write_finished_default_layout_is_byte_exact() {
        // The on-wire output is fully specified: [4-byte LE len | payload] per
        // frame, concatenated. Assert the exact bytes for a 3-frame stream.
        let mut wire = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut wire), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();

        let mut expected = Vec::new();
        for i in 0..3 {
            let payload = finished(&mut builder, &format!("message {i}"));
            writer.write_finished(&mut builder).unwrap();
            expected.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            expected.extend_from_slice(&payload);
        }
        writer.flush().unwrap();
        drop(writer);
        assert_eq!(wire, expected);
    }

    #[cfg(feature = "xxhash")]
    #[test]
    fn write_finished_checksummed_layout_is_byte_exact() {
        // [4-byte LE len | 8-byte LE xxh3 | payload], checksum over the payload
        // only. Recompute the checksum independently and assert exact bytes.
        let mut wire = Vec::new();
        let mut writer =
            StreamWriter::new(Cursor::new(&mut wire), ChecksumFramer::new(XxHash64::new()));
        let mut builder = FlatBufferBuilder::new();
        let payload = finished(&mut builder, "test data");
        writer.write_finished(&mut builder).unwrap();
        writer.flush().unwrap();
        drop(writer);

        let mut expected = Vec::new();
        expected.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        expected.extend_from_slice(&XxHash64::new().calculate(&payload).to_le_bytes());
        expected.extend_from_slice(&payload);
        assert_eq!(wire, expected);
    }

    #[test]
    fn simple_mode_writes_readable_string_root() {
        // Simple mode serializes through the internal builder; the framed
        // payload must parse back as the same string root.
        let mut wire = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut wire), DefaultFramer);
        writer.write(&"test message").unwrap();
        writer.flush().unwrap();
        drop(writer);

        let len = u32::from_le_bytes(wire[..4].try_into().unwrap()) as usize;
        assert_eq!(wire.len(), 4 + len);
        let root = flatbuffers::root::<&str>(&wire[4..]).unwrap();
        assert_eq!(root, "test message");
    }

    #[test]
    fn write_with_policy_installed_is_transparent() {
        // An installed no-op policy must not change the bytes written.
        let mut without = Vec::new();
        StreamWriter::new(Cursor::new(&mut without), DefaultFramer)
            .write(&"policy message")
            .unwrap();

        let mut with_policy = Vec::new();
        StreamWriter::new(Cursor::new(&mut with_policy), DefaultFramer)
            .with_memory_policy(NoOpPolicy)
            .write(&"policy message")
            .unwrap();

        assert_eq!(with_policy, without);
    }

    #[test]
    fn writer_with_policy_is_send() {
        fn assert_send<T: Send>(_: &T) {}
        let writer =
            StreamWriter::new(std::io::sink(), DefaultFramer).with_memory_policy(NoOpPolicy);
        assert_send(&writer);
    }

    #[test]
    fn receipts_match_default_wire_layout() {
        // Each receipt must name the exact byte range the frame occupies:
        // frame_start accumulates, wire_len == 4-byte len prefix + payload.
        let mut wire = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut wire), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();

        let mut expected_start = 0u64;
        for i in 0..3 {
            let payload = finished(&mut builder, &format!("message {i}"));
            let receipt = writer.write_finished_with_receipt(&mut builder).unwrap();
            assert_eq!(receipt.frame_start, expected_start);
            assert_eq!(receipt.wire_len, (4 + payload.len()) as u64);
            assert_eq!(
                writer.bytes_written(),
                receipt.frame_start + receipt.wire_len
            );
            expected_start += receipt.wire_len;
        }
        assert_eq!(writer.bytes_written(), expected_start);
    }

    #[test]
    fn receipt_names_the_exact_bytes_written() {
        // A receipt's [frame_start, frame_start + wire_len) must index exactly
        // the bytes the writer produced for that frame.
        let mut wire = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut wire), DefaultFramer);
        let r0 = writer.write_with_receipt(&"first").unwrap();
        let r1 = writer.write_with_receipt(&"second").unwrap();
        writer.flush().unwrap();
        drop(writer);

        assert_eq!(r0.frame_start, 0);
        assert_eq!(r1.frame_start, r0.wire_len);
        assert_eq!((r0.wire_len + r1.wire_len) as usize, wire.len());
        // The recorded len prefix inside each frame agrees with wire_len.
        let l0 = u32::from_le_bytes(wire[..4].try_into().unwrap()) as u64;
        assert_eq!(r0.wire_len, 4 + l0);
    }

    #[test]
    fn with_start_offset_shifts_receipts_and_position() {
        // A writer positioned over a nonzero file region reports absolute
        // offsets when told its start offset.
        let mut wire = Vec::new();
        let mut writer =
            StreamWriter::new(Cursor::new(&mut wire), DefaultFramer).with_start_offset(1000);
        assert_eq!(writer.bytes_written(), 1000);
        let r = writer.write_with_receipt(&"shifted").unwrap();
        assert_eq!(r.frame_start, 1000);
        assert_eq!(writer.bytes_written(), 1000 + r.wire_len);
    }

    #[cfg(feature = "xxhash")]
    #[test]
    fn checksummed_receipt_wire_len_includes_checksum() {
        // wire_len must account for the 8-byte checksum field, not just len +
        // payload — the receipt reflects the framer's actual output.
        let mut wire = Vec::new();
        let mut writer =
            StreamWriter::new(Cursor::new(&mut wire), ChecksumFramer::new(XxHash64::new()));
        let mut builder = FlatBufferBuilder::new();
        let payload = finished(&mut builder, "checked");
        let r = writer.write_finished_with_receipt(&mut builder).unwrap();
        assert_eq!(r.wire_len, (4 + XxHash64::SIZE + payload.len()) as u64);
    }

    #[test]
    fn owned_stream_writer_alias_hides_lifetime() {
        // The whole point of the alias: a write_finished-only writer needs no
        // lifetime annotation. A function returning the alias must accept the
        // value `new()` produces.
        fn make<W: Write>(w: W) -> OwnedStreamWriter<W, DefaultFramer> {
            StreamWriter::new(w, DefaultFramer)
        }
        let mut wire = Vec::new();
        let mut writer = make(Cursor::new(&mut wire));
        let mut builder = FlatBufferBuilder::new();
        let _ = finished(&mut builder, "aliased");
        writer.write_finished(&mut builder).unwrap();
        writer.flush().unwrap();
        drop(writer);
        assert!(!wire.is_empty());
    }
}
