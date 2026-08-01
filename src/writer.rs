//! A generic, composable writer for `flatstream`.

use crate::durability::{Durable, NoSync, SyncMode, SyncPolicy, SyncPolicyBackend, Syncing};
use crate::error::{Error, Result};
use crate::framing::Framer;
use crate::policy::{MemoryPolicy, NoMemoryPolicy, ReclamationInfo};
use crate::traits::StreamSerialize;
use flatbuffers::{DefaultAllocator, FlatBufferBuilder};
use std::io::Write;
use std::time::{Duration, Instant};

/// Builds a fresh internal builder after a memory policy requests reclamation.
pub trait BuilderFactory<'a, A: flatbuffers::Allocator>: Send {
    fn make_builder(&mut self, capacity: usize) -> FlatBufferBuilder<'a, A>;
}

impl<'a, A, F> BuilderFactory<'a, A> for F
where
    A: flatbuffers::Allocator,
    F: FnMut(usize) -> FlatBufferBuilder<'a, A> + Send,
{
    fn make_builder(&mut self, capacity: usize) -> FlatBufferBuilder<'a, A> {
        self(capacity)
    }
}

/// Zero-sized factory for the default FlatBuffers allocator.
#[derive(Debug, Clone, Copy, Default)]
pub struct DefaultBuilderFactory;

impl<'a> BuilderFactory<'a, DefaultAllocator> for DefaultBuilderFactory {
    fn make_builder(&mut self, capacity: usize) -> FlatBufferBuilder<'a, DefaultAllocator> {
        FlatBufferBuilder::with_capacity(capacity)
    }
}

/// Statically dispatched writer memory-policy state.
// The derives apply when the policy and builder factory are themselves
// `Debug`/`Clone` (closure factories usually are not).
#[derive(Debug, Clone)]
pub struct WriterMemoryPolicy<P, B> {
    policy: P,
    baseline_capacity: usize,
    make_builder: B,
}

impl<P: MemoryPolicy, B> WriterMemoryPolicy<P, B> {
    fn new(policy: P, make_builder: B) -> Self {
        let baseline_capacity = policy.baseline_capacity();
        Self {
            policy,
            baseline_capacity,
            make_builder,
        }
    }
}

/// Internal static-dispatch bridge for writer-owned memory.
#[doc(hidden)]
pub trait WriterMemoryBackend<'a, A: flatbuffers::Allocator>: Send {
    fn after_write(&mut self, builder: &mut FlatBufferBuilder<'a, A>, last_message_size: usize);
}

impl<'a, A: flatbuffers::Allocator> WriterMemoryBackend<'a, A> for NoMemoryPolicy {
    #[inline(always)]
    fn after_write(&mut self, _builder: &mut FlatBufferBuilder<'a, A>, _last_message_size: usize) {}
}

impl<'a, A, P, B> WriterMemoryBackend<'a, A> for WriterMemoryPolicy<P, B>
where
    A: flatbuffers::Allocator,
    P: MemoryPolicy,
    B: BuilderFactory<'a, A>,
{
    #[inline]
    fn after_write(&mut self, builder: &mut FlatBufferBuilder<'a, A>, last_message_size: usize) {
        let current_capacity = builder.mut_finished_buffer().0.len();
        if current_capacity <= self.baseline_capacity {
            return;
        }
        if let Some(reason) = self
            .policy
            .should_reset(last_message_size, current_capacity)
        {
            *builder = self.make_builder.make_builder(self.baseline_capacity);
            self.policy.on_reclaim(&ReclamationInfo {
                reason,
                last_message_size,
                capacity_before: current_capacity,
                capacity_after: self.baseline_capacity,
            });
        }
    }
}

/// Wraps the underlying writer and advances a stream position by bytes actually
/// accepted, so a `StreamWriter` can report each [`FrameReceipt`] without the
/// `Framer` trait reporting lengths. The position starts at zero or a
/// caller-supplied append offset and stays exact for custom framers too.
struct CountingWriter<W> {
    inner: W,
    /// Absolute stream position. A nonzero initial value is installed before
    /// I/O through `StreamWriter::with_start_offset`.
    count: u64,
    position_overflowed: bool,
}

impl<W> CountingWriter<W> {
    #[inline]
    fn new(inner: W) -> Self {
        Self {
            inner,
            count: 0,
            position_overflowed: false,
        }
    }

    fn set_start_offset(&mut self, offset: u64) -> Result<()> {
        if self.count != 0 || self.position_overflowed {
            return Err(Error::invalid_frame(
                "start offset must be configured before any stream I/O",
            ));
        }
        self.count = offset;
        Ok(())
    }

    #[inline]
    fn record_accepted(&mut self, accepted: usize) -> std::io::Result<()> {
        match self.count.checked_add(accepted as u64) {
            Some(position) => {
                self.count = position;
                Ok(())
            }
            None => {
                // The sink has already accepted these bytes. Preserve the
                // largest representable position, fail the operation, and let
                // StreamWriter poison itself so no later receipt can wrap.
                self.count = u64::MAX;
                self.position_overflowed = true;
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "stream position exceeds u64",
                ))
            }
        }
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
        let remaining = usize::try_from(u64::MAX - self.count).unwrap_or(usize::MAX);
        if !buf.is_empty() && remaining == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stream position exceeds u64",
            ));
        }
        let n = self.inner.write(&buf[..buf.len().min(remaining)])?;
        self.record_accepted(n)?;
        Ok(n)
    }

    /// Overriding the vectored path preserves E1's single-syscall framing; it is
    /// not required for correctness: the provided fallback calls this wrapper's
    /// counted `write`. The override delegates native vectoring to the inner
    /// sink so built-in framing retains its one-call shape.
    ///
    /// `write_all` is intentionally *not* overridden. Its provided
    /// implementation loops through this wrapper's `write`, which means bytes
    /// accepted before a later error remain visible in `bytes_written`.
    #[inline]
    fn write_vectored(&mut self, bufs: &[std::io::IoSlice<'_>]) -> std::io::Result<usize> {
        let remaining = u64::MAX - self.count;
        let requested: u128 = bufs.iter().map(|buf| buf.len() as u128).sum();
        if requested > remaining as u128 {
            if let Some(first) = bufs.iter().find(|buf| !buf.is_empty()) {
                return self.write(first);
            }
            return Ok(0);
        }
        let n = self.inner.write_vectored(bufs)?;
        self.record_accepted(n)?;
        Ok(n)
    }

    #[inline]
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Zero-sized default state for writers without post-operation observation.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoPostWriteObserver;

/// The completed outcome of one [`StreamWriter`] write operation.
#[derive(Debug)]
pub enum PostWriteOutcome<'a> {
    /// Serialization, framing, and any automatic durability checkpoint
    /// completed successfully.
    Succeeded(FrameReceipt),
    /// The caller's [`StreamSerialize`] implementation failed before framing.
    SerializationFailed(&'a Error),
    /// Framing or sink I/O failed.
    ///
    /// `bytes_accepted` is exact even when a custom framer uses
    /// [`Write::write_all`] and the sink fails after a partial write. A nonzero
    /// value poisons the writer; consume and recover/truncate the sink before
    /// constructing a replacement writer.
    WriteFailed {
        /// Position at which the attempted frame began.
        frame_start: u64,
        /// Bytes accepted before the failure.
        bytes_accepted: u64,
        /// Error returned to the caller.
        error: &'a Error,
    },
    /// The frame was accepted, but its automatic durability checkpoint failed.
    ///
    /// The receipt identifies the accepted frame; callers must not re-emit it.
    DurabilityFailed {
        receipt: FrameReceipt,
        error: &'a Error,
    },
}

/// Post-operation information delivered to a [`PostWriteObserver`].
#[derive(Debug)]
pub struct PostWriteEvent<'a> {
    /// Serialized payload length. `None` when serialization failed or when a
    /// previously poisoned writer rejected the operation before serialization.
    pub payload_len: Option<usize>,
    /// Elapsed time through completion of the operation, excluding the
    /// observer callback itself.
    pub elapsed: Duration,
    /// Final operation outcome.
    pub outcome: PostWriteOutcome<'a>,
}

/// Statically dispatched observer invoked after a writer operation resolves.
///
/// Installing an observer adds one monotonic-clock pair and the concrete
/// callback per write. The default [`NoPostWriteObserver`] is zero-sized and
/// disables timing entirely at compile time.
pub trait PostWriteObserver: Send {
    /// Whether this observer requires event collection. The default is `true`;
    /// the zero-sized default overrides it so the compiler removes timing and
    /// callback work from unobserved writers.
    const ENABLED: bool = true;

    /// Receives exactly one event after each `write*` operation returns its
    /// final success/failure state. The callback is not invoked pre-I/O.
    fn on_write(&mut self, event: PostWriteEvent<'_>);
}

impl PostWriteObserver for NoPostWriteObserver {
    const ENABLED: bool = false;

    #[inline(always)]
    fn on_write(&mut self, _event: PostWriteEvent<'_>) {}
}

impl<F> PostWriteObserver for F
where
    F: for<'event> FnMut(PostWriteEvent<'event>) + Send,
{
    fn on_write(&mut self, event: PostWriteEvent<'_>) {
        self(event);
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
/// ```
/// use flatstream::{DefaultFramer, StreamWriter};
/// use flatstream::policy::AdaptiveWatermarkPolicy;
///
/// # let file = Vec::new();
/// let writer = StreamWriter::new(file, DefaultFramer)
///     .with_memory_policy(AdaptiveWatermarkPolicy::new(4, 5).with_baseline(16 * 1024));
/// # let _ = writer;
/// ```
///
/// Policy state is a generic parameter. The default [`NoMemoryPolicy`] is
/// zero-sized and its backend call compiles away; installed policies are
/// monomorphized. **Policies apply to simple mode only**: in expert mode
/// (`write_finished()`) the caller owns the builder, so the writer cannot and
/// does not reclaim it.
///
/// ## Durability policies
///
/// The default [`NoSync`] state is zero-sized and performs no policy check.
/// For a [`Durable`] sink, [`with_sync_policy`](Self::with_sync_policy) changes
/// the writer's concrete type to `Syncing<P>` and evaluates the statically
/// dispatched policy after each complete frame. Successful checkpoints update
/// [`durable_watermark`](StreamWriter::durable_watermark); manual
/// [`sync_data`](StreamWriter::sync_data) / [`sync_all`](StreamWriter::sync_all)
/// return the same byte position.
///
/// A durability error occurs after the triggering frame has been accepted.
/// [`crate::ErrorKind::DurabilityFailed`] carries
/// the attempted and previous watermarks plus the frame coordinates so callers
/// do not duplicate the frame by blindly retrying it.
///
/// ## Post-write observation
///
/// [`with_post_write_observer`](Self::with_post_write_observer) installs a
/// concrete callback that runs after serialization, framing/I/O, memory-policy
/// bookkeeping, and any automatic durability checkpoint resolve. The default
/// [`NoPostWriteObserver`] is zero-sized and enables no timing or callback work.
///
/// ## Failed writes are fail-stop
///
/// If framing fails after the sink accepted any bytes, the stream ends in a
/// partial frame. The writer records those bytes, reports them through
/// [`PostWriteOutcome::WriteFailed`], and becomes poisoned: later writes and
/// durability checkpoints are rejected with
/// [`ErrorKind::Poisoned`](crate::ErrorKind::Poisoned), and
/// [`is_poisoned`](Self::is_poisoned) reports the state without provoking it.
/// Consume it with [`into_inner`](Self::into_inner),
/// stop all writing, recover/truncate the torn tail, and construct a new writer
/// at the recovered offset. An error that accepted zero bytes does not poison
/// the writer and may be retried.
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
pub struct StreamWriter<
    'a,
    W: Write,
    F: Framer,
    A = DefaultAllocator,
    S = NoSync,
    M = NoMemoryPolicy,
    O = NoPostWriteObserver,
> where
    A: flatbuffers::Allocator,
{
    writer: CountingWriter<W>,
    framer: F,
    builder: FlatBufferBuilder<'a, A>,
    memory: M,
    sync: S,
    observer: O,
    /// A failed frame that accepted bytes destroys append alignment. Keep the
    /// writer fail-stop until the sink is consumed and recovered.
    poisoned: bool,
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
pub type OwnedStreamWriter<W, F, S = NoSync, M = NoMemoryPolicy, O = NoPostWriteObserver> =
    StreamWriter<'static, W, F, DefaultAllocator, S, M, O>;

/// The byte position and on-wire size of one frame, returned by receipt-aware
/// write and read APIs.
///
/// `frame_start` is the offset of the frame's first byte; `wire_len` is the
/// total bytes the frame occupies on the wire (length prefix + optional
/// checksum + payload). The next frame begins at `frame_start + wire_len`.
/// Offsets are relative to the writer's start offset (0 by default, or the value
/// given to [`StreamWriter::with_start_offset`] for a writer positioned over a
/// nonzero region of a file), so they can be recorded in an external index and
/// used to seek a reader — no `8 + payload_len` wire arithmetic in caller code.
///
/// Receipts are plain values: hashable for index keys, and ordered by
/// `frame_start` (then `wire_len`) — stream order for receipts from one stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FrameReceipt {
    /// Offset of the frame's first byte, measured from the writer's start
    /// offset — so it is an absolute file offset whenever
    /// [`with_start_offset`](StreamWriter::with_start_offset) was given one,
    /// and a stream-relative offset otherwise (the default start offset is 0).
    pub frame_start: u64,
    /// Total bytes the frame occupies on the wire.
    pub wire_len: u64,
}

impl FrameReceipt {
    /// Returns the exact end offset when the coordinates are representable.
    pub const fn checked_end(&self) -> Option<u64> {
        self.frame_start.checked_add(self.wire_len)
    }

    /// Offset one past this frame's final byte.
    ///
    /// Receipts produced by flatstream are checked while bytes are counted and
    /// never overflow. A manually constructed receipt with invalid coordinates
    /// saturates at `u64::MAX`; use [`checked_end`](Self::checked_end) when
    /// validating externally supplied receipt fields.
    pub const fn end(&self) -> u64 {
        self.frame_start.saturating_add(self.wire_len)
    }

    /// The frame's exact byte range on the wire.
    pub const fn range(&self) -> std::ops::Range<u64> {
        self.frame_start..self.end()
    }
}

impl<'a, W: Write, F: Framer> StreamWriter<'a, W, F, DefaultAllocator, NoSync, NoMemoryPolicy> {
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
            memory: NoMemoryPolicy,
            sync: NoSync,
            observer: NoPostWriteObserver,
            poisoned: false,
        }
    }

    /// Creates a new `StreamWriter` with a pre-constructed builder.
    /// Useful for pre-sizing.
    pub fn with_builder(writer: W, framer: F, builder: FlatBufferBuilder<'a>) -> Self {
        Self {
            writer: CountingWriter::new(writer),
            framer,
            builder,
            memory: NoMemoryPolicy,
            sync: NoSync,
            observer: NoPostWriteObserver,
            poisoned: false,
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
            memory: NoMemoryPolicy,
            sync: NoSync,
            observer: NoPostWriteObserver,
            poisoned: false,
        }
    }
}

impl<'a, W: Write, F: Framer, S, M, O> StreamWriter<'a, W, F, DefaultAllocator, S, M, O> {
    /// Installs a memory reclamation policy on this writer (simple mode only).
    ///
    /// After each successful `write()`, the policy observes the message size and
    /// current builder capacity; when it fires, the internal builder is replaced
    /// with a fresh one at the policy's baseline capacity.
    #[must_use]
    pub fn with_memory_policy<P: MemoryPolicy>(
        self,
        policy: P,
    ) -> StreamWriter<'a, W, F, DefaultAllocator, S, WriterMemoryPolicy<P, DefaultBuilderFactory>, O>
    {
        StreamWriter {
            writer: self.writer,
            framer: self.framer,
            builder: self.builder,
            memory: WriterMemoryPolicy::new(policy, DefaultBuilderFactory),
            sync: self.sync,
            observer: self.observer,
            poisoned: self.poisoned,
        }
    }
}

impl<'a, W: Write, F: Framer, A> StreamWriter<'a, W, F, A, NoSync, NoMemoryPolicy>
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
    /// ```
    /// use flatbuffers::FlatBufferBuilder;
    /// use flatstream::{DefaultFramer, StreamWriter};
    ///
    /// // `DefaultAllocator` stands in for yours; any `flatbuffers::Allocator`
    /// // works the same way.
    /// let allocator = flatbuffers::DefaultAllocator::default();
    /// let builder = FlatBufferBuilder::new_in(allocator);
    /// let writer = StreamWriter::with_builder_alloc(Vec::new(), DefaultFramer, builder);
    /// # let _ = writer;
    /// ```
    pub fn with_builder_alloc(writer: W, framer: F, builder: FlatBufferBuilder<'a, A>) -> Self {
        Self {
            writer: CountingWriter::new(writer),
            framer,
            builder,
            memory: NoMemoryPolicy,
            sync: NoSync,
            observer: NoPostWriteObserver,
            poisoned: false,
        }
    }
}

impl<'a, W: Write, F: Framer, A, S, M, O> StreamWriter<'a, W, F, A, S, M, O>
where
    A: flatbuffers::Allocator,
    S: SyncPolicyBackend<W>,
    M: WriterMemoryBackend<'a, A>,
    O: PostWriteObserver,
{
    /// Installs a memory reclamation policy together with a builder factory.
    ///
    /// This is the custom-allocator variant of
    /// [`with_memory_policy`](Self::with_memory_policy): a reclaim replaces the
    /// internal builder with `make_builder(policy.baseline_capacity())`, so the
    /// factory decides how a fresh builder (and its allocator) is constructed.
    #[must_use]
    pub fn with_memory_policy_and_factory<P, B>(
        self,
        policy: P,
        make_builder: B,
    ) -> StreamWriter<'a, W, F, A, S, WriterMemoryPolicy<P, B>, O>
    where
        P: MemoryPolicy,
        B: BuilderFactory<'a, A>,
    {
        StreamWriter {
            writer: self.writer,
            framer: self.framer,
            builder: self.builder,
            memory: WriterMemoryPolicy::new(policy, make_builder),
            sync: self.sync,
            observer: self.observer,
            poisoned: self.poisoned,
        }
    }

    #[inline(always)]
    fn observation_start() -> Option<Instant> {
        if O::ENABLED {
            Some(Instant::now())
        } else {
            None
        }
    }

    #[inline]
    fn observe_write(
        &mut self,
        started: Option<Instant>,
        payload_len: Option<usize>,
        outcome: PostWriteOutcome<'_>,
    ) {
        if let Some(started) = started {
            self.observer.on_write(PostWriteEvent {
                payload_len,
                elapsed: started.elapsed(),
                outcome,
            });
        }
    }

    #[inline]
    fn reject_if_poisoned(
        &mut self,
        started: Option<Instant>,
        payload_len: Option<usize>,
    ) -> Result<()> {
        if !self.poisoned {
            return Ok(());
        }
        let error = Error::poisoned();
        let frame_start = self.writer.count;
        self.observe_write(
            started,
            payload_len,
            PostWriteOutcome::WriteFailed {
                frame_start,
                bytes_accepted: 0,
                error: &error,
            },
        );
        Err(error)
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
    /// ```
    /// use flatstream::{DefaultFramer, StreamWriter};
    ///
    /// # fn main() -> flatstream::Result<()> {
    /// let mut writer = StreamWriter::new(Vec::new(), DefaultFramer);
    /// writer.write(&"Hello, world!")?;
    /// writer.write(&"another message")?;
    /// writer.flush()?;
    /// # Ok(())
    /// # }
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
        let started = Self::observation_start();
        self.reject_if_poisoned(started, None)?;

        // Reset the internal builder for reuse
        self.builder.reset();

        // Serialize directly into the reusable builder. The implementation of
        // StreamSerialize controls any temporary work it performs.
        if let Err(error) = item.serialize(&mut self.builder) {
            self.observe_write(started, None, PostWriteOutcome::SerializationFailed(&error));
            return Err(error);
        }

        // Get the finished payload from the builder
        let payload = self.builder.finished_data();
        let last_message_size = payload.len();

        // Delegate framing and writing to the strategy, bracketing it with the
        // byte counter so the receipt reflects exactly what reached the wire.
        let frame_start = self.writer.count;
        if let Err(error) = self.framer.frame_and_write(&mut self.writer, payload) {
            let bytes_accepted = self.writer.count.saturating_sub(frame_start);
            if bytes_accepted != 0 || self.writer.position_overflowed {
                self.poisoned = true;
            }
            self.observe_write(
                started,
                Some(last_message_size),
                PostWriteOutcome::WriteFailed {
                    frame_start,
                    bytes_accepted,
                    error: &error,
                },
            );
            return Err(error);
        }
        let wire_len = self.writer.count - frame_start;

        // Static dispatch: `NoMemoryPolicy` compiles this call away.
        self.memory
            .after_write(&mut self.builder, last_message_size);

        let receipt = FrameReceipt {
            frame_start,
            wire_len,
        };
        if let Err(error) = self.sync.after_frame(self.writer.get_mut(), receipt) {
            let outcome = PostWriteOutcome::DurabilityFailed {
                receipt,
                error: &error,
            };
            self.observe_write(started, Some(last_message_size), outcome);
            return Err(error);
        }
        self.observe_write(
            started,
            Some(last_message_size),
            PostWriteOutcome::Succeeded(receipt),
        );
        Ok(receipt)
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
    /// ```
    /// use flatbuffers::FlatBufferBuilder;
    /// use flatstream::{DefaultFramer, StreamSerialize, StreamWriter};
    ///
    /// # fn main() -> flatstream::Result<()> {
    /// let mut writer = StreamWriter::new(Vec::new(), DefaultFramer);
    /// let mut builder = FlatBufferBuilder::new();
    /// let events = ["first", "second"];
    ///
    /// for event in events {
    ///     builder.reset(); // Critical: reuse allocated memory!
    ///     event.serialize(&mut builder)?;
    ///     writer.write_finished(&mut builder)?;
    /// }
    /// # Ok(())
    /// # }
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
        let started = Self::observation_start();

        // Get the finished payload from the builder
        let payload = builder.finished_data();
        let payload_len = payload.len();
        self.reject_if_poisoned(started, Some(payload_len))?;

        // Delegate framing and writing to the strategy, bracketing it with the
        // byte counter so the receipt reflects exactly what reached the wire.
        let frame_start = self.writer.count;
        if let Err(error) = self.framer.frame_and_write(&mut self.writer, payload) {
            let bytes_accepted = self.writer.count.saturating_sub(frame_start);
            if bytes_accepted != 0 || self.writer.position_overflowed {
                self.poisoned = true;
            }
            self.observe_write(
                started,
                Some(payload_len),
                PostWriteOutcome::WriteFailed {
                    frame_start,
                    bytes_accepted,
                    error: &error,
                },
            );
            return Err(error);
        }
        let wire_len = self.writer.count - frame_start;

        let receipt = FrameReceipt {
            frame_start,
            wire_len,
        };
        if let Err(error) = self.sync.after_frame(self.writer.get_mut(), receipt) {
            let outcome = PostWriteOutcome::DurabilityFailed {
                receipt,
                error: &error,
            };
            self.observe_write(started, Some(payload_len), outcome);
            return Err(error);
        }
        self.observe_write(
            started,
            Some(payload_len),
            PostWriteOutcome::Succeeded(receipt),
        );
        Ok(receipt)
    }

    /// Flushes the underlying writer.
    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }

    /// Consumes the stream, returning the underlying writer.
    ///
    /// This is the only mutable escape hatch: out-of-band I/O cannot occur
    /// while receipt accounting is active. After raw writes, construct a new
    /// `StreamWriter` and set its absolute base with
    /// [`with_start_offset`](Self::with_start_offset).
    pub fn into_inner(self) -> W {
        self.writer.into_inner()
    }

    /// Returns a reference to the framer strategy.
    pub fn framer(&self) -> &F {
        &self.framer
    }

    /// The current absolute or stream-relative offset. Equivalently, the
    /// `frame_start` the next written frame will receive. After a partial frame
    /// failure it includes every byte the sink accepted before the writer was
    /// poisoned.
    pub fn bytes_written(&self) -> u64 {
        self.writer.count
    }

    /// Whether an earlier failed frame poisoned this writer.
    ///
    /// A poisoned writer is fail-stop: the sink holds a partial frame, so its
    /// next byte is no longer a frame boundary, and writes, durability
    /// checkpoints, and rebasing are rejected with
    /// [`ErrorKind::Poisoned`](crate::ErrorKind::Poisoned). Recover by
    /// consuming the writer with [`into_inner`](Self::into_inner),
    /// recovering/truncating the torn tail, and constructing a replacement at
    /// the recovered offset. An error that accepted zero bytes does not
    /// poison, so `false` after a failed write means the write may be retried.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Installs a statically dispatched post-write observer.
    ///
    /// The observer receives one event after each `write*` operation reaches
    /// its final state, including serialization errors, sink/framing failures,
    /// and automatic durability failures after frame acceptance. Installing an
    /// observer enables per-operation monotonic timing; the default
    /// [`NoPostWriteObserver`] performs neither clock reads nor callbacks.
    #[must_use]
    pub fn with_post_write_observer<O2: PostWriteObserver>(
        self,
        observer: O2,
    ) -> StreamWriter<'a, W, F, A, S, M, O2> {
        StreamWriter {
            writer: self.writer,
            framer: self.framer,
            builder: self.builder,
            memory: self.memory,
            sync: self.sync,
            observer,
            poisoned: self.poisoned,
        }
    }

    /// Sets the offset that [`FrameReceipt`] offsets and
    /// [`bytes_written`](Self::bytes_written) are measured from.
    ///
    /// Use it when the underlying writer is positioned over a nonzero region of
    /// a file (e.g. appending to an existing journal) and you want receipts to
    /// carry absolute file offsets. Defaults to 0. This returns an error if any
    /// stream I/O has already occurred, preventing a rebase from invalidating
    /// existing receipts or durable watermarks.
    pub fn with_start_offset(mut self, offset: u64) -> Result<Self> {
        if self.poisoned {
            return Err(Error::poisoned());
        }
        self.writer.set_start_offset(offset)?;
        Ok(self)
    }
}

impl<'a, W: Durable, F: Framer, A, M, O> StreamWriter<'a, W, F, A, NoSync, M, O>
where
    A: flatbuffers::Allocator,
    M: WriterMemoryBackend<'a, A>,
    O: PostWriteObserver,
{
    /// Installs a statically dispatched durability policy.
    ///
    /// Policy installation is available only on the default [`NoSync`] state,
    /// so it cannot silently replace a live policy or discard an established
    /// durable watermark. Frame-, byte-, and interval-based policies compose
    /// statically through [`crate::SyncPolicyExt::or`].
    #[must_use]
    pub fn with_sync_policy<P: SyncPolicy>(
        self,
        policy: P,
    ) -> StreamWriter<'a, W, F, A, Syncing<P>, M, O> {
        StreamWriter {
            writer: self.writer,
            framer: self.framer,
            builder: self.builder,
            memory: self.memory,
            sync: Syncing::new(policy),
            observer: self.observer,
            poisoned: self.poisoned,
        }
    }

    /// Flushes buffered bytes and synchronizes file contents.
    ///
    /// Returns the durable watermark. Retain it if you need to test receipts
    /// later; default no-policy writers intentionally store no watermark state.
    pub fn sync_data(&mut self) -> Result<u64> {
        self.manual_sync_without_policy(SyncMode::Data)
    }

    /// Flushes buffered bytes and synchronizes file contents and metadata.
    pub fn sync_all(&mut self) -> Result<u64> {
        self.manual_sync_without_policy(SyncMode::All)
    }

    fn manual_sync_without_policy(&mut self, mode: SyncMode) -> Result<u64> {
        if self.poisoned {
            return Err(Error::poisoned());
        }
        let attempted_watermark = self.bytes_written();
        let result = match mode {
            SyncMode::Data => self.writer.get_mut().sync_data(),
            SyncMode::All => self.writer.get_mut().sync_all(),
        };
        result.map_err(|source| {
            Error::durability_failed(mode, attempted_watermark, None, None, None, source)
        })?;
        Ok(attempted_watermark)
    }
}

impl<'a, W: Durable, F: Framer, A, P, M, O> StreamWriter<'a, W, F, A, Syncing<P>, M, O>
where
    A: flatbuffers::Allocator,
    P: SyncPolicy,
    M: WriterMemoryBackend<'a, A>,
    O: PostWriteObserver,
{
    /// Last successfully synchronized stream offset, or `None` before the
    /// first successful checkpoint.
    pub fn durable_watermark(&self) -> Option<u64> {
        self.sync.durable_watermark
    }

    /// Forces a content checkpoint and resets the installed policy window.
    pub fn sync_data(&mut self) -> Result<u64> {
        self.manual_sync(SyncMode::Data)
    }

    /// Forces a content-and-metadata checkpoint and resets the policy window.
    pub fn sync_all(&mut self) -> Result<u64> {
        self.manual_sync(SyncMode::All)
    }

    fn manual_sync(&mut self, mode: SyncMode) -> Result<u64> {
        if self.poisoned {
            return Err(Error::poisoned());
        }
        let attempted_watermark = self.bytes_written();
        self.sync
            .checkpoint(self.writer.get_mut(), mode, attempted_watermark, None)
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
        let mut writer = StreamWriter::new(Cursor::new(&mut wire), DefaultFramer)
            .with_start_offset(1000)
            .unwrap();
        assert_eq!(writer.bytes_written(), 1000);
        let r = writer.write_with_receipt(&"shifted").unwrap();
        assert_eq!(r.frame_start, 1000);
        assert_eq!(writer.bytes_written(), 1000 + r.wire_len);
    }

    #[test]
    fn invalid_external_receipt_coordinates_saturate_instead_of_wrapping() {
        let receipt = FrameReceipt {
            frame_start: u64::MAX,
            wire_len: 1,
        };
        assert_eq!(receipt.checked_end(), None);
        assert_eq!(receipt.end(), u64::MAX);
        assert_eq!(receipt.range(), u64::MAX..u64::MAX);
    }

    /// A sink whose first write fails outright (zero bytes accepted) and then
    /// behaves normally — the transient-device shape the fail-stop contract
    /// distinguishes from a torn frame.
    struct FailFirstWrite {
        written: Vec<u8>,
        failed_once: bool,
    }

    impl Write for FailFirstWrite {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if !self.failed_once {
                self.failed_once = true;
                return Err(std::io::Error::other("transient device error"));
            }
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn zero_byte_failure_does_not_poison_and_the_write_retries() {
        // The other half of the fail-stop contract: an error that accepted
        // zero bytes leaves the stream at a frame boundary, so the writer
        // stays usable and the same write succeeds on retry.
        let mut writer = StreamWriter::new(
            FailFirstWrite {
                written: Vec::new(),
                failed_once: false,
            },
            DefaultFramer,
        );
        assert!(!writer.is_poisoned());

        let error = writer
            .write(&"retry me")
            .expect_err("first write must fail");
        assert!(matches!(error.kind(), crate::error::ErrorKind::Io(_)));
        assert!(!writer.is_poisoned(), "zero accepted bytes must not poison");
        assert_eq!(writer.bytes_written(), 0);

        let receipt = writer.write_with_receipt(&"retry me").unwrap();
        assert_eq!(receipt.frame_start, 0);
        assert_eq!(writer.bytes_written(), receipt.wire_len);
        assert_eq!(writer.into_inner().written.len() as u64, receipt.wire_len);
    }

    #[test]
    fn start_offset_cannot_rebase_an_active_writer() {
        let mut writer = StreamWriter::new(Vec::new(), DefaultFramer);
        writer.write(&"already written").unwrap();
        let error = match writer.with_start_offset(1_000) {
            Ok(_) => panic!("rebasing after accepted bytes must fail"),
            Err(error) => error,
        };
        assert!(matches!(
            error.kind(),
            crate::error::ErrorKind::InvalidFrame { .. }
        ));
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

    /// A sink that implements `write_vectored` and reports it — the shape of
    /// a real `File`/`TcpStream`. It records vectored vs. scalar calls
    /// separately so a test can tell which path a frame actually took: dropping
    /// `CountingWriter::write_vectored` would silently route frames through the
    /// scalar `write` fallback — still byte- and receipt-correct, but no longer
    /// a single vectored syscall.
    struct VectoringSink {
        written: Vec<u8>,
        vectored_calls: usize,
        scalar_calls: usize,
    }

    impl Write for VectoringSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.scalar_calls += 1;
            self.written.extend_from_slice(buf);
            Ok(buf.len())
        }
        fn write_vectored(&mut self, bufs: &[std::io::IoSlice<'_>]) -> std::io::Result<usize> {
            self.vectored_calls += 1;
            let mut total = 0;
            for buf in bufs {
                self.written.extend_from_slice(buf);
                total += buf.len();
            }
            Ok(total)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn receipts_are_correct_for_a_vectoring_sink() {
        // What this test really guards is the *call shape*, not the offsets.
        // The framers emit each frame as one `write_vectored`. If
        // `CountingWriter` dropped its `write_vectored` override, `Write`'s
        // provided implementation would forward to this wrapper's own `write`,
        // so the byte count — and every receipt and `bytes_written()` — would
        // stay exactly correct. What would change, silently, is that each frame
        // would take two scalar `write`s instead of one vectored call,
        // reverting E1's syscall win with no failing receipt to notice it.
        // Hence the assertions below check both the offsets *and* that the sink
        // saw vectored calls and no scalar ones. (Empirically: removing the
        // override fails only the `vectored_calls == 3` assertion below; every
        // receipt assertion still passes.)
        let sink = VectoringSink {
            written: Vec::new(),
            vectored_calls: 0,
            scalar_calls: 0,
        };
        let mut writer = StreamWriter::new(sink, DefaultFramer);
        let mut builder = FlatBufferBuilder::new();

        let mut expected_start = 0u64;
        for i in 0..3 {
            let payload = finished(&mut builder, &format!("frame {i}"));
            let receipt = writer.write_finished_with_receipt(&mut builder).unwrap();
            assert_eq!(receipt.frame_start, expected_start);
            assert_eq!(receipt.wire_len, (4 + payload.len()) as u64);
            expected_start += receipt.wire_len;
        }

        assert_eq!(writer.bytes_written(), expected_start);
        let sink = writer.into_inner();
        // The receipt total must equal the bytes the sink actually received,
        // and the frames must genuinely have gone down the vectored path.
        assert_eq!(sink.written.len() as u64, expected_start);
        assert_eq!(sink.vectored_calls, 3, "one vectored call per frame");
        assert_eq!(sink.scalar_calls, 0, "no scalar writes on a vectoring sink");
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
