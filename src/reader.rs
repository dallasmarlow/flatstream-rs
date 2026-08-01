//! A generic, composable reader for `flatstream`.

use crate::error::{Error, Result};
use crate::framing::{Deframer, RetrySafeDeframer};
use crate::policy::{MemoryPolicy, NoMemoryPolicy, ReclamationInfo};
use crate::traits::StreamDeserialize;
use crate::writer::FrameReceipt;
use std::io::{IoSliceMut, Read, Seek, SeekFrom};
use std::marker::PhantomData;

/// Wraps a source and advances a stream position by bytes actually returned
/// through `Read`.
///
/// Deframers are generic over `Read`, so routing them through this wrapper
/// provides frame positions without changing the `Deframer` trait.
struct CountingReader<R> {
    inner: R,
    /// Absolute stream position after an optional start offset is installed.
    count: u64,
    position_overflowed: bool,
}

impl<R> CountingReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            count: 0,
            position_overflowed: false,
        }
    }

    fn with_position(inner: R, position: u64) -> Self {
        Self {
            inner,
            count: position,
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
    fn record_consumed(&mut self, consumed: usize) -> std::io::Result<()> {
        match self.count.checked_add(consumed as u64) {
            Some(position) => {
                self.count = position;
                Ok(())
            }
            None => {
                self.count = u64::MAX;
                self.position_overflowed = true;
                Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "stream position exceeds u64",
                ))
            }
        }
    }

    fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for CountingReader<R> {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let remaining = usize::try_from(u64::MAX - self.count).unwrap_or(usize::MAX);
        if !buf.is_empty() && remaining == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stream position exceeds u64",
            ));
        }
        let allowed = buf.len().min(remaining);
        let n = self.inner.read(&mut buf[..allowed])?;
        self.record_consumed(n)?;
        Ok(n)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> std::io::Result<usize> {
        let remaining = u64::MAX - self.count;
        let requested: u128 = bufs.iter().map(|buf| buf.len() as u128).sum();
        if requested > remaining as u128 {
            if let Some(first) = bufs.iter_mut().find(|buf| !buf.is_empty()) {
                return self.read(first);
            }
            return Ok(0);
        }
        let n = self.inner.read_vectored(bufs)?;
        self.record_consumed(n)?;
        Ok(n)
    }
}

/// One successfully decoded frame and its exact wire bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadFrame<'a> {
    /// Payload borrowed from caller-owned or reader-owned scratch storage.
    pub payload: &'a [u8],
    /// Absolute or stream-relative wire range, matching the reader's configured
    /// start offset.
    pub receipt: FrameReceipt,
}

/// Statically dispatched reader memory-policy state.
// The derives apply when the policy is itself `Debug`/`Clone`.
#[derive(Debug, Clone)]
pub struct ReaderMemoryPolicy<P> {
    policy: P,
    baseline_capacity: usize,
    pending_reclaim: Option<PendingReclamation>,
}

#[derive(Debug, Clone, Copy)]
struct PendingReclamation {
    reason: crate::policy::ReclamationReason,
    last_message_size: usize,
    capacity_before: usize,
}

impl<P: MemoryPolicy> ReaderMemoryPolicy<P> {
    fn new(policy: P) -> Self {
        let baseline_capacity = policy.baseline_capacity();
        Self {
            policy,
            baseline_capacity,
            pending_reclaim: None,
        }
    }
}

/// Internal static-dispatch bridge for reader-owned memory.
#[doc(hidden)]
pub trait ReaderMemoryBackend: Send {
    fn before_read(&mut self, buffer: &mut Vec<u8>);
    fn after_read(&mut self, buffer_capacity: usize, last_message_size: usize);
}

impl ReaderMemoryBackend for NoMemoryPolicy {
    #[inline(always)]
    fn before_read(&mut self, _buffer: &mut Vec<u8>) {}

    #[inline(always)]
    fn after_read(&mut self, _buffer_capacity: usize, _last_message_size: usize) {}
}

impl<P: MemoryPolicy> ReaderMemoryBackend for ReaderMemoryPolicy<P> {
    #[inline]
    fn before_read(&mut self, buffer: &mut Vec<u8>) {
        if let Some(pending) = self.pending_reclaim.take() {
            // The previous payload borrow has ended by the time the next read
            // reaches this hook, so the existing allocation may now be
            // reclaimed. `shrink_to` can still call the allocator (and may move
            // the allocation); memory-policy reclamation is an intentional
            // cold event, not part of the zero-allocation steady-state claim.
            buffer.clear();
            buffer.shrink_to(self.baseline_capacity);
            self.policy.on_reclaim(&ReclamationInfo {
                reason: pending.reason,
                last_message_size: pending.last_message_size,
                capacity_before: pending.capacity_before,
                capacity_after: buffer.capacity(),
            });
        }
    }

    #[inline]
    fn after_read(&mut self, buffer_capacity: usize, last_message_size: usize) {
        if buffer_capacity <= self.baseline_capacity {
            return;
        }
        if let Some(reason) = self.policy.should_reset(last_message_size, buffer_capacity) {
            self.pending_reclaim = Some(PendingReclamation {
                reason,
                last_message_size,
                capacity_before: buffer_capacity,
            });
        }
    }
}

/// Reads exactly one frame beginning at absolute `offset`.
///
/// The payload is decoded into caller-owned `scratch` and borrowed from it in
/// the result. Once `scratch` reaches the largest frame used by a workload,
/// repeated lookups allocate nothing. Bounds and checksum behavior come from
/// `deframer`, exactly as they do for [`StreamReader`].
///
/// On success, `src` is positioned one byte past the frame. `Ok(None)` means
/// clean EOF at `offset`. An EOF observed after any part of a frame is
/// [`ErrorKind::UnexpectedEof`](crate::ErrorKind::UnexpectedEof); a live-file
/// follower may retry safely by calling this function again with the same
/// absolute offset, which seeks back before parsing. The [`RetrySafeDeframer`]
/// bound makes the corresponding deframer-state guarantee explicit.
///
/// Each call performs one initial seek. The frame's `wire_len` is counted from
/// bytes actually returned by `Read`, so no post-read position query is needed.
///
/// Passing a buffered reader is legal, but every point lookup seeks and
/// invalidates its buffered position. Even so, buffering can reduce syscall
/// count for small frames; benchmark a retained `BufReader<File>` against a bare
/// `File` for the application's frame-size distribution.
pub fn read_frame_at<'a, R, D>(
    src: &mut R,
    deframer: &D,
    offset: u64,
    scratch: &'a mut Vec<u8>,
) -> Result<Option<ReadFrame<'a>>>
where
    R: Read + Seek,
    D: RetrySafeDeframer,
{
    src.seek(SeekFrom::Start(offset))?;
    // Count the deframer's actual reads instead of asking the seekable source
    // for its position afterward. On File, a second `stream_position()` would
    // be another syscall on every point lookup; the read count already is the
    // frame's exact wire length.
    let mut reader = CountingReader::with_position(src, offset);
    let Some(payload_len) = deframer.read_and_deframe(&mut reader, scratch)? else {
        if reader.count != offset {
            return Err(Error::unexpected_eof());
        }
        return Ok(None);
    };
    if payload_len > scratch.len() {
        return Err(Error::invalid_frame(
            "deframer returned a payload length beyond its scratch buffer",
        ));
    }
    Ok(Some(ReadFrame {
        payload: &scratch[..payload_len],
        receipt: FrameReceipt {
            frame_start: offset,
            wire_len: reader.count - offset,
        },
    }))
}

/// A reader for streaming messages from a `flatstream`.
///
/// This reader is generic over a `Deframer` strategy, which defines how
/// each message is parsed from the byte stream.
///
/// **Copy behavior**: Both APIs yield `&[u8]` slices borrowed from the internal
/// buffer. A generic [`Read`] source copies each frame once into that buffer;
/// growth may allocate, while warmed high-water-mark processing adds no
/// allocation, second payload copy, or deserialization. The deframer parses
/// only the frame header (length and optional checksum), never the payload,
/// which is handed to the caller as the serialized FlatBuffer bytes.
///
/// The returned `&[u8]` payload slices are borrowed from the reader's
/// internal buffer and are valid only until the next successful read.
///
/// It provides two APIs:
/// 1. **Processor API** (`process_all()`): High-performance closure-based processing
/// 2. **Expert API** (`messages()`): Manual iteration for maximum control
///
/// # Performance: Processor API vs. Expert API
///
/// The `process_all()` method uses a closure that receives borrowed slices
/// (`&[u8]`) directly from the internal buffer. After the buffer reaches its
/// high-water mark, the read loop performs no heap allocations and adds no
/// second payload copy.
///
/// The `messages()` method provides manual iteration control for cases where you
/// need more complex control flow or want to process messages conditionally.
/// Performance: Same as `process_all()` - both use zero-copy access.
///
/// Every successful frame also has a [`FrameReceipt`] available through
/// [`read_message_with_receipt`](Self::read_message_with_receipt),
/// [`process_all_with_receipt`](Self::process_all_with_receipt), or
/// [`Messages::next_with_receipt`]. [`bytes_consumed`](Self::bytes_consumed)
/// reports the next frame boundary. For indexed scatter reads, [`read_frame_at`]
/// uses caller-owned scratch without constructing a `StreamReader`.
///
/// A sequential read error that consumed frame bytes poisons the reader because
/// its next byte is no longer a known frame boundary. Later reads are rejected
/// with [`ErrorKind::Poisoned`](crate::ErrorKind::Poisoned), and
/// [`is_poisoned`](Self::is_poisoned) reports the state without provoking it;
/// consume and reconstruct the reader at a verified offset. Positioned retries
/// use [`read_frame_at`] and require a [`RetrySafeDeframer`].
///
/// ```rust
/// # use flatstream::{StreamReader, DefaultDeframer, Result};
/// # use std::io::Cursor;
/// # let mut reader = StreamReader::new(Cursor::new(vec![]), DefaultDeframer::new());
///
/// // High-performance processor API
/// reader.process_all(|payload| {
///     // Process payload directly (zero-copy)
///     println!("Message: {} bytes", payload.len());
///     Ok(())
/// })?;
///
/// // Expert API for manual control
/// let mut messages = reader.messages();
/// while let Some(payload) = messages.next()? {
///     // Process payload directly (zero-copy)
///     println!("Message: {} bytes", payload.len());
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
///
/// ## Buffer Behavior and Frame Bounds
///
/// The internal buffer is a high-water mark: it grows to the largest payload
/// seen (zero-initializing only the growth) and is then reused in place, so
/// steady-state reads perform no allocation and no per-frame zeroing. By
/// default the deframers accept declared lengths up to the FlatBuffers
/// maximum buffer size
/// ([`DEFAULT_MAX_FRAME_LEN`](crate::framing::DEFAULT_MAX_FRAME_LEN), 2 GiB);
/// when reading from an untrusted source, tighten this with
/// `DefaultDeframer::new().with_max_frame_len(max)` so a corrupt header is
/// rejected *before* any allocation is sized from it.
///
/// ## Memory Reclamation
///
/// The internal buffer grows to the largest message seen and keeps that
/// capacity. For long-running processes with bursty workloads, an optional
/// [`MemoryPolicy`] can be installed with
/// [`with_memory_policy`](Self::with_memory_policy) to shrink the buffer back
/// to the policy's baseline capacity (`MemoryPolicy::baseline_capacity`,
/// default 16 KiB). The shrink is deferred to the start of the next read, so a
/// payload already returned is never invalidated. Policy state is generic and
/// statically dispatched; the zero-sized [`NoMemoryPolicy`] default compiles
/// away.
pub struct StreamReader<R: Read, D: Deframer, M = NoMemoryPolicy> {
    reader: CountingReader<R>,
    deframer: D,
    // The reader owns its buffer, resizing as needed.
    buffer: Vec<u8>,
    memory: M,
    /// A failed read that consumed bytes leaves sequential alignment
    /// indeterminate. Require reconstruction before another read.
    poisoned: bool,
}

impl<R: Read, D: Deframer> StreamReader<R, D, NoMemoryPolicy> {
    /// Creates a new `StreamReader` with the given reader and deframing strategy.
    pub fn new(reader: R, deframer: D) -> Self {
        Self {
            reader: CountingReader::new(reader),
            deframer,
            buffer: Vec::new(),
            memory: NoMemoryPolicy,
            poisoned: false,
        }
    }

    /// Creates a new `StreamReader` with a pre-allocated buffer capacity.
    pub fn with_capacity(reader: R, deframer: D, capacity: usize) -> Self {
        Self {
            reader: CountingReader::new(reader),
            deframer,
            buffer: Vec::with_capacity(capacity),
            memory: NoMemoryPolicy,
            poisoned: false,
        }
    }
}

impl<R: Read, D: Deframer, M: ReaderMemoryBackend> StreamReader<R, D, M> {
    /// Installs a memory reclamation policy on this reader.
    ///
    /// After each successful read, the policy observes the payload size and the
    /// internal buffer's capacity; when it fires, the existing buffer is cleared
    /// and asked to shrink to the policy's baseline capacity
    /// (`MemoryPolicy::baseline_capacity`, cached here at installation). The
    /// shrink is deferred to the start of the *next* read so the payload just
    /// returned is never invalidated. `Vec::shrink_to` may reallocate and may
    /// retain some excess capacity; reclamation is an intentional cold event,
    /// outside the zero-allocation steady-state guarantee. The policy is
    /// consulted only while the buffer's capacity exceeds the baseline — at or
    /// below it there is nothing to reclaim.
    #[must_use]
    pub fn with_memory_policy<P: MemoryPolicy>(
        self,
        policy: P,
    ) -> StreamReader<R, D, ReaderMemoryPolicy<P>> {
        StreamReader {
            reader: self.reader,
            deframer: self.deframer,
            buffer: self.buffer,
            memory: ReaderMemoryPolicy::new(policy),
            poisoned: self.poisoned,
        }
    }

    /// Reads the next message into the internal buffer. This is the low-level
    /// alternative to using the processor or expert APIs.
    /// Returns Ok(Some(payload)) on success, Ok(None) on clean EOF.
    ///
    /// Memory policy dispatch is static; the default [`NoMemoryPolicy`] calls
    /// below inline away.
    #[inline]
    pub fn read_message(&mut self) -> Result<Option<&[u8]>> {
        match self.read_message_with_receipt()? {
            Some(frame) => Ok(Some(frame.payload)),
            None => Ok(None),
        }
    }

    /// Reads one message and returns its exact wire bounds.
    ///
    /// The receipt is measured from this reader's configured start offset. It
    /// includes the length prefix, optional checksum, and payload, so callers
    /// can construct or verify indexes without duplicating framing arithmetic.
    #[inline]
    pub fn read_message_with_receipt(&mut self) -> Result<Option<ReadFrame<'_>>> {
        if self.poisoned {
            return Err(Error::poisoned());
        }
        self.memory.before_read(&mut self.buffer);
        let frame_start = self.bytes_consumed();
        let result = self
            .deframer
            .read_and_deframe(&mut self.reader, &mut self.buffer);
        match result {
            Ok(Some(n)) => {
                if n > self.buffer.len() {
                    self.poisoned = true;
                    return Err(Error::invalid_frame(
                        "deframer returned a payload length beyond its buffer",
                    ));
                }
                self.memory.after_read(self.buffer.capacity(), n);
                let wire_len = self.bytes_consumed() - frame_start;
                Ok(Some(ReadFrame {
                    payload: &self.buffer[..n],
                    receipt: FrameReceipt {
                        frame_start,
                        wire_len,
                    },
                }))
            }
            Ok(None) => {
                if self.bytes_consumed() != frame_start {
                    self.poisoned = true;
                    return Err(Error::unexpected_eof());
                }
                Ok(None)
            }
            Err(error) => {
                if self.bytes_consumed() != frame_start || self.reader.position_overflowed {
                    self.poisoned = true;
                }
                Err(error)
            }
        }
    }

    /// Processes all messages in the stream using a closure.
    ///
    /// This is the highest-performance API, providing zero-copy access to message
    /// payloads through borrowed slices (`&[u8]`). The closure receives each message
    /// payload and should return `Ok(())` to continue processing or an error to stop.
    ///
    /// # Arguments
    /// * `processor` - A closure that processes each message payload
    ///
    /// # Returns
    /// * `Ok(())` - All messages processed successfully
    /// * `Err(e)` - An error occurred during processing or reading
    pub fn process_all<F>(&mut self, mut processor: F) -> Result<()>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        while let Some(payload) = self.read_message()? {
            // Process the payload using the user's closure
            processor(payload)?;
        }
        Ok(())
    }

    /// Processes every frame with its exact wire receipt.
    pub fn process_all_with_receipt<F>(&mut self, mut processor: F) -> Result<()>
    where
        for<'p> F: FnMut(ReadFrame<'p>) -> Result<()>,
    {
        while let Some(frame) = self.read_message_with_receipt()? {
            processor(frame)?;
        }
        Ok(())
    }

    /// Returns an iterator-like object for manual message processing.
    ///
    /// This provides the "expert path" for users who need more control over
    /// the iteration process. Each call to `next_message()` returns a borrowed slice
    /// to the message payload, providing zero-copy access.
    ///
    /// Lifetimes: Each returned payload `&[u8]` is valid only until the next successful read.
    pub fn messages(&mut self) -> Messages<'_, R, D, M> {
        Messages { reader: self }
    }

    /// Returns a typed iterator-like object for manual message processing.
    ///
    /// This yields verified FlatBuffer roots using the `StreamDeserialize` trait
    /// while preserving zero-copy lifetimes tied to the reader.
    pub fn typed_messages<T>(&mut self) -> TypedMessages<'_, R, D, T, M>
    where
        for<'p> T: StreamDeserialize<'p>,
    {
        TypedMessages {
            reader: self,
            _phantom: PhantomData,
        }
    }

    /// Processes all messages in the stream, automatically deserializing them
    /// into a strongly-typed FlatBuffer root object.
    ///
    /// This method combines the high-performance, zero-copy `process_all`
    /// with the type-safe deserialization provided by the `StreamDeserialize` trait.
    /// It removes boilerplate and adds compile-time type safety to the reading path.
    ///
    /// # Type Parameters
    /// * `T`: A type that implements `StreamDeserialize<'_>`, representing the
    ///   expected FlatBuffer root type (e.g., `MyEvent`).
    /// * `F`: A closure that processes the strongly-typed FlatBuffer root object.
    ///
    /// # Arguments
    /// * `processor` - A closure that receives the deserialized FlatBuffer root object.
    ///   It should return `Ok(())` to continue processing or an error to stop.
    ///
    /// ```rust
    /// # use flatstream::*;
    /// # use std::io::Cursor;
    /// struct StrRoot;
    /// impl<'a> StreamDeserialize<'a> for StrRoot {
    ///     type Root = &'a str;
    ///     fn from_payload(payload: &'a [u8]) -> Result<Self::Root> {
    ///         flatbuffers::root::<&'a str>(payload).map_err(Error::from)
    ///     }
    /// }
    ///
    /// # fn main() -> Result<()> {
    /// // Write one string root
    /// let mut buf = Vec::new();
    /// {
    ///     let mut writer = StreamWriter::new(Cursor::new(&mut buf), DefaultFramer);
    ///     let mut builder = flatbuffers::FlatBufferBuilder::new();
    ///     let s = builder.create_string("hello");
    ///     builder.finish(s, None);
    ///     writer.write_finished(&mut builder)?;
    /// }
    ///
    /// // Read with typed API
    /// let mut reader = StreamReader::new(Cursor::new(&buf), DefaultDeframer::new());
    /// reader.process_typed::<StrRoot, _>(|root| {
    ///     assert_eq!(root, "hello");
    ///     Ok(())
    /// })?;
    /// Ok(())
    /// # }
    /// ```
    pub fn process_typed<T, F>(&mut self, mut processor: F) -> Result<()>
    where
        for<'p> T: StreamDeserialize<'p>,
        for<'p> F: FnMut(<T as StreamDeserialize<'p>>::Root) -> Result<()>,
    {
        self.process_all(|payload| {
            let root = <T as StreamDeserialize<'_>>::from_payload(payload)?;
            processor(root)
        })
    }

    /// Processes all messages using unchecked FlatBuffer root access.
    ///
    /// # Safety
    ///
    /// Every payload in the stream must be a valid FlatBuffer for the expected
    /// `T::Root`. This method skips FlatBuffers verification; invalid bytes may
    /// cause panics or undefined behavior when followed by generated accessors.
    #[cfg(feature = "unsafe_typed")]
    pub unsafe fn process_typed_unchecked<T, F>(&mut self, mut processor: F) -> Result<()>
    where
        for<'p> T: StreamDeserialize<'p>,
        for<'p> F: FnMut(
            <<T as StreamDeserialize<'p>>::Root as flatbuffers::Follow<'p>>::Inner,
        ) -> Result<()>,
    {
        self.process_all(|payload| {
            let inner = unsafe {
                flatbuffers::root_unchecked::<<T as StreamDeserialize<'_>>::Root>(payload)
            };
            processor(inner)
        })
    }

    /// Processes all messages and passes both the typed root and raw payload.
    pub fn process_typed_with_payload<T, F>(&mut self, mut processor: F) -> Result<()>
    where
        for<'p> T: StreamDeserialize<'p>,
        for<'p> F: FnMut(<T as StreamDeserialize<'p>>::Root, &'p [u8]) -> Result<()>,
    {
        self.process_all(|payload| {
            let root = <T as StreamDeserialize<'_>>::from_payload(payload)?;
            processor(root, payload)
        })
    }

    /// Current absolute or stream-relative source position.
    ///
    /// After a successful frame this is the offset where the next frame begins.
    /// If a read fails mid-frame, it reflects bytes already consumed by that
    /// failed attempt.
    pub fn bytes_consumed(&self) -> u64 {
        self.reader.count
    }

    /// Whether an earlier failed read poisoned this reader.
    ///
    /// A poisoned reader is fail-stop: a sequential read that consumed part of
    /// a frame and then failed leaves the source inside that frame, so later
    /// reads are rejected with
    /// [`ErrorKind::Poisoned`](crate::ErrorKind::Poisoned). Consume the reader
    /// with [`into_inner`](Self::into_inner) and reconstruct it at a verified
    /// offset; positioned lookups retry through [`read_frame_at`] instead,
    /// which seeks back to the frame start on every call. An error that
    /// consumed zero bytes does not poison, so `false` after a failed read
    /// means the read may be retried.
    pub fn is_poisoned(&self) -> bool {
        self.poisoned
    }

    /// Sets the base used by [`bytes_consumed`](Self::bytes_consumed) and
    /// receipts returned by [`read_message_with_receipt`](Self::read_message_with_receipt).
    ///
    /// Set this before reading when the wrapped source begins at a nonzero
    /// stream position. Calling it after any read returns an error rather than
    /// silently rebasing subsequent receipts.
    pub fn with_start_offset(mut self, offset: u64) -> Result<Self> {
        if self.poisoned {
            return Err(Error::poisoned());
        }
        self.reader.set_start_offset(offset)?;
        Ok(self)
    }

    /// Returns a reference to the deframer strategy.
    pub fn deframer(&self) -> &D {
        &self.deframer
    }

    /// Returns the current capacity of the internal buffer.
    pub fn buffer_capacity(&self) -> usize {
        self.buffer.capacity()
    }

    /// Ensure the internal buffer can hold at least `additional` more bytes without reallocation.
    pub fn reserve(&mut self, additional: usize) {
        self.buffer.reserve(additional)
    }

    /// Consumes the stream, returning the underlying reader.
    ///
    /// Mutable out-of-band reads/seeks require consuming the reader so position
    /// accounting cannot silently become stale. Construct a new `StreamReader`
    /// afterward and use [`with_start_offset`](Self::with_start_offset) when
    /// receipts should remain absolute.
    pub fn into_inner(self) -> R {
        self.reader.into_inner()
    }
}

/// An iterator-like object for manual message processing.
///
/// This struct provides the "expert path" for users who need more control over
/// the iteration process. It borrows the `StreamReader` mutably, ensuring
/// proper lifetime management.
pub struct Messages<'a, R: Read, D: Deframer, M = NoMemoryPolicy> {
    reader: &'a mut StreamReader<R, D, M>,
}

impl<'a, R: Read, D: Deframer, M: ReaderMemoryBackend> Messages<'a, R, D, M> {
    /// Returns the next message in the stream.
    ///
    /// # Returns
    /// * `Ok(Some(payload))` - A message was successfully read
    /// * `Ok(None)` - End of stream reached
    /// * `Err(e)` - An error occurred during reading
    #[inline]
    pub fn next_message(&mut self) -> Result<Option<&[u8]>> {
        self.reader.read_message()
    }

    /// Returns the next message with its exact wire receipt.
    #[inline]
    pub fn next_with_receipt(&mut self) -> Result<Option<ReadFrame<'_>>> {
        self.reader.read_message_with_receipt()
    }

    #[allow(clippy::should_implement_trait)]
    #[inline]
    pub fn next(&mut self) -> Result<Option<&[u8]>> {
        self.next_message()
    }
}

/// Typed iterator-like object yielding verified FlatBuffer roots.
pub struct TypedMessages<'a, R: Read, D: Deframer, T, M = NoMemoryPolicy>
where
    for<'p> T: StreamDeserialize<'p>,
{
    reader: &'a mut StreamReader<R, D, M>,
    _phantom: PhantomData<T>,
}

impl<'a, R: Read, D: Deframer, T, M: ReaderMemoryBackend> TypedMessages<'a, R, D, T, M>
where
    for<'p> T: StreamDeserialize<'p>,
{
    /// Returns the next typed root in the stream.
    ///
    /// ```rust
    /// # use flatstream::*;
    /// # use std::io::Cursor;
    /// struct StrRoot;
    /// impl<'a> StreamDeserialize<'a> for StrRoot {
    ///     type Root = &'a str;
    ///     fn from_payload(payload: &'a [u8]) -> Result<Self::Root> {
    ///         flatbuffers::root::<&'a str>(payload).map_err(Error::from)
    ///     }
    /// }
    /// # fn main() -> Result<()> {
    /// let mut buf = Vec::new();
    /// {
    ///     let mut w = StreamWriter::new(Cursor::new(&mut buf), DefaultFramer);
    ///     let mut b = flatbuffers::FlatBufferBuilder::new();
    ///     let s = b.create_string("hello");
    ///     b.finish(s, None);
    ///     w.write_finished(&mut b)?;
    /// }
    /// let mut r = StreamReader::new(Cursor::new(&buf), DefaultDeframer::new());
    /// let mut it = r.typed_messages::<StrRoot>();
    /// let first = it.next().unwrap().unwrap();
    /// assert_eq!(first, "hello");
    /// # Ok(()) }
    /// ```
    #[inline]
    pub fn next_typed<'p>(&'p mut self) -> Result<Option<<T as StreamDeserialize<'p>>::Root>> {
        match self.reader.read_message()? {
            Some(payload) => {
                let root = <T as StreamDeserialize<'p>>::from_payload(payload)?;
                Ok(Some(root))
            }
            None => Ok(None),
        }
    }

    #[allow(clippy::should_implement_trait)]
    #[inline]
    pub fn next<'p>(&'p mut self) -> Result<Option<<T as StreamDeserialize<'p>>::Root>> {
        self.next_typed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::DefaultDeframer;
    use crate::framing::DefaultFramer;
    use crate::writer::StreamWriter;
    use flatbuffers::FlatBufferBuilder;

    #[cfg(feature = "xxhash")]
    use crate::{ChecksumDeframer, ChecksumFramer, XxHash64};
    use std::io::Cursor;
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct RecordingPolicy(Arc<Mutex<Vec<ReclamationInfo>>>);

    impl MemoryPolicy for RecordingPolicy {
        fn should_reset(
            &mut self,
            _last_message_size: usize,
            _current_capacity: usize,
        ) -> Option<crate::policy::ReclamationReason> {
            None
        }

        fn on_reclaim(&mut self, info: &ReclamationInfo) {
            self.0.lock().unwrap().push(*info);
        }

        fn baseline_capacity(&self) -> usize {
            64
        }
    }

    /// Writes `messages` as string roots and returns (wire bytes, expected
    /// payload bytes per frame).
    fn write_stream<F: crate::framing::Framer>(
        framer: F,
        messages: &[&str],
    ) -> (Vec<u8>, Vec<Vec<u8>>) {
        let mut wire = Vec::new();
        let mut expected = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut wire), framer);
        let mut builder = FlatBufferBuilder::new();
        for msg in messages {
            builder.reset();
            let data = builder.create_string(msg);
            builder.finish(data, None);
            expected.push(builder.finished_data().to_vec());
            writer.write_finished(&mut builder).unwrap();
        }
        drop(writer);
        (wire, expected)
    }

    #[test]
    fn read_message_returns_exact_payload() {
        let (wire, expected) = write_stream(DefaultFramer, &["test data"]);
        let mut reader = StreamReader::new(Cursor::new(wire), DefaultDeframer::new());
        assert_eq!(reader.read_message().unwrap().unwrap(), &expected[0][..]);
        assert!(reader.read_message().unwrap().is_none());
    }

    #[cfg(feature = "xxhash")]
    #[test]
    fn read_message_checksummed_returns_exact_payload() {
        let (wire, expected) = write_stream(ChecksumFramer::new(XxHash64::new()), &["test data"]);
        let mut reader =
            StreamReader::new(Cursor::new(wire), ChecksumDeframer::new(XxHash64::new()));
        assert_eq!(reader.read_message().unwrap().unwrap(), &expected[0][..]);
        assert!(reader.read_message().unwrap().is_none());
    }

    #[test]
    fn process_all_yields_every_payload_in_order() {
        let (wire, expected) = write_stream(DefaultFramer, &["one", "two", "three"]);
        let mut reader = StreamReader::new(Cursor::new(wire), DefaultDeframer::new());
        let mut count = 0usize;
        reader
            .process_all(|payload| {
                assert_eq!(payload, &expected[count][..]);
                count += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(count, expected.len());
    }

    #[test]
    fn messages_iterator_yields_every_payload_in_order() {
        let (wire, expected) = write_stream(DefaultFramer, &["one", "two", "three"]);
        let mut reader = StreamReader::new(Cursor::new(wire), DefaultDeframer::new());
        let mut count = 0usize;
        let mut messages = reader.messages();
        while let Some(payload) = messages.next().unwrap() {
            assert_eq!(payload, &expected[count][..]);
            count += 1;
        }
        assert_eq!(count, expected.len());
    }

    #[test]
    fn empty_stream_is_clean_eof_on_both_apis() {
        let mut reader = StreamReader::new(Cursor::new(Vec::new()), DefaultDeframer::new());
        assert!(reader.read_message().unwrap().is_none());

        let mut reader = StreamReader::new(Cursor::new(Vec::new()), DefaultDeframer::new());
        let mut count = 0usize;
        reader
            .process_all(|_| {
                count += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn pending_reader_reclamation_shrinks_the_existing_buffer_once() {
        let mut buffer = Vec::with_capacity(4096);
        buffer.resize(1024, 0xA5);
        let original_capacity = buffer.capacity();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut memory = ReaderMemoryPolicy {
            policy: RecordingPolicy(Arc::clone(&events)),
            baseline_capacity: 64,
            pending_reclaim: Some(PendingReclamation {
                reason: crate::policy::ReclamationReason::SizeThreshold,
                last_message_size: 16,
                capacity_before: original_capacity,
            }),
        };
        assert!(
            events.lock().unwrap().is_empty(),
            "scheduling alone must not report a reclaim"
        );

        memory.before_read(&mut buffer);

        assert!(buffer.is_empty(), "old payload bytes are no longer exposed");
        assert!(
            buffer.capacity() >= 64 && buffer.capacity() <= original_capacity,
            "shrink_to keeps at least the baseline without growing capacity"
        );
        assert!(memory.pending_reclaim.is_none());
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].capacity_before, original_capacity);
        assert_eq!(events[0].capacity_after, buffer.capacity());
        drop(events);

        let reclaimed_capacity = buffer.capacity();
        memory.before_read(&mut buffer);
        assert_eq!(
            buffer.capacity(),
            reclaimed_capacity,
            "a completed reclamation is not repeated"
        );
    }

    /// A source whose first read fails before returning any byte, then
    /// delegates — the transient-device shape that must stay retryable.
    struct FailFirstRead<R> {
        inner: R,
        failed_once: bool,
    }

    impl<R: std::io::Read> std::io::Read for FailFirstRead<R> {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if !self.failed_once {
                self.failed_once = true;
                return Err(std::io::Error::other("transient device error"));
            }
            self.inner.read(buf)
        }
    }

    #[test]
    fn zero_byte_failure_does_not_poison_and_the_read_retries() {
        // The other half of the fail-stop contract: an error that consumed
        // zero bytes leaves the source at a frame boundary, so the reader
        // stays usable and the same read succeeds on retry.
        let (wire, expected) = write_stream(DefaultFramer, &["retry me"]);
        let mut reader = StreamReader::new(
            FailFirstRead {
                inner: Cursor::new(wire),
                failed_once: false,
            },
            DefaultDeframer::new(),
        );
        assert!(!reader.is_poisoned());

        let error = reader.read_message().expect_err("first read must fail");
        assert!(matches!(error.kind(), crate::error::ErrorKind::Io(_)));
        assert!(!reader.is_poisoned(), "zero consumed bytes must not poison");
        assert_eq!(reader.bytes_consumed(), 0);

        assert_eq!(reader.read_message().unwrap().unwrap(), &expected[0][..]);
        assert!(reader.read_message().unwrap().is_none());
    }

    #[test]
    fn start_offset_cannot_rebase_an_active_reader() {
        let (wire, _) = write_stream(DefaultFramer, &["already read"]);
        let mut reader = StreamReader::new(Cursor::new(wire), DefaultDeframer::new());
        reader.read_message().unwrap().unwrap();
        let error = match reader.with_start_offset(1_000) {
            Ok(_) => panic!("rebasing after consumed bytes must fail"),
            Err(error) => error,
        };
        assert!(matches!(
            error.kind(),
            crate::error::ErrorKind::InvalidFrame { .. }
        ));
    }

    #[test]
    fn process_all_propagates_processor_error_and_stops() {
        // A processor error must stop iteration immediately and surface intact.
        let (wire, _) = write_stream(DefaultFramer, &["a", "b", "c", "d", "e"]);
        let mut reader = StreamReader::new(Cursor::new(wire), DefaultDeframer::new());
        let mut count = 0usize;
        let result = reader.process_all(|_| {
            count += 1;
            if count == 3 {
                return Err(crate::error::Error::from(std::io::Error::other(
                    "Simulated processing error",
                )));
            }
            Ok(())
        });
        assert_eq!(count, 3);
        match result.unwrap_err().into_kind() {
            crate::error::ErrorKind::Io(e) => {
                assert_eq!(e.kind(), std::io::ErrorKind::Other);
                assert_eq!(e.to_string(), "Simulated processing error");
            }
            _ => panic!("Expected Io error"),
        }
    }
}
