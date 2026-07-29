//! A generic, composable reader for `flatstream`.

use crate::error::Result;
use crate::framing::Deframer;
use crate::policy::{MemoryPolicy, NoMemoryPolicy, ReclamationInfo};
use crate::traits::StreamDeserialize;
use crate::writer::FrameReceipt;
use std::io::{IoSliceMut, Read, Seek, SeekFrom};
use std::marker::PhantomData;

/// Wraps a source and counts bytes actually returned through `Read`.
///
/// Deframers are generic over `Read`, so routing them through this wrapper
/// provides frame positions without changing the `Deframer` trait.
struct CountingReader<R> {
    inner: R,
    count: u64,
}

impl<R> CountingReader<R> {
    fn new(inner: R) -> Self {
        Self { inner, count: 0 }
    }

    fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: Read> Read for CountingReader<R> {
    #[inline]
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count += n as u64;
        Ok(n)
    }

    #[inline]
    fn read_vectored(&mut self, bufs: &mut [IoSliceMut<'_>]) -> std::io::Result<usize> {
        let n = self.inner.read_vectored(bufs)?;
        self.count += n as u64;
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
pub struct ReaderMemoryPolicy<P> {
    policy: P,
    baseline_capacity: usize,
    pending_shrink: bool,
}

impl<P: MemoryPolicy> ReaderMemoryPolicy<P> {
    fn new(policy: P) -> Self {
        let baseline_capacity = policy.baseline_capacity();
        Self {
            policy,
            baseline_capacity,
            pending_shrink: false,
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
        if self.pending_shrink {
            // The previous payload borrow has ended by the time the next read
            // reaches this hook, so the existing allocation may now be
            // reclaimed. `shrink_to` can still call the allocator (and may move
            // the allocation); memory-policy reclamation is an intentional
            // cold event, not part of the zero-allocation steady-state claim.
            buffer.clear();
            buffer.shrink_to(self.baseline_capacity);
            self.pending_shrink = false;
        }
    }

    #[inline]
    fn after_read(&mut self, buffer_capacity: usize, last_message_size: usize) {
        if buffer_capacity <= self.baseline_capacity {
            return;
        }
        if let Some(reason) = self.policy.should_reset(last_message_size, buffer_capacity) {
            self.pending_shrink = true;
            self.policy.on_reclaim(&ReclamationInfo {
                reason,
                last_message_size,
                capacity_before: buffer_capacity,
                capacity_after: self.baseline_capacity,
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
/// absolute offset, which seeks back before parsing.
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
    D: Deframer,
{
    src.seek(SeekFrom::Start(offset))?;
    // Count the deframer's actual reads instead of asking the seekable source
    // for its position afterward. On File, a second `stream_position()` would
    // be another syscall on every point lookup; the read count already is the
    // frame's exact wire length.
    let mut reader = CountingReader::new(src);
    let Some(payload_len) = deframer.read_and_deframe(&mut reader, scratch)? else {
        return Ok(None);
    };
    Ok(Some(ReadFrame {
        payload: &scratch[..payload_len],
        receipt: FrameReceipt {
            frame_start: offset,
            wire_len: reader.count,
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
    // This addresses Lesson 4 and 16 for memory efficiency.
    buffer: Vec<u8>,
    memory: M,
    /// Base added to counted source bytes. Set before reading when the source
    /// begins at a nonzero position.
    start_offset: u64,
}

impl<R: Read, D: Deframer> StreamReader<R, D, NoMemoryPolicy> {
    /// Creates a new `StreamReader` with the given reader and deframing strategy.
    pub fn new(reader: R, deframer: D) -> Self {
        Self {
            reader: CountingReader::new(reader),
            deframer,
            buffer: Vec::new(),
            memory: NoMemoryPolicy,
            start_offset: 0,
        }
    }

    /// Creates a new `StreamReader` with a pre-allocated buffer capacity.
    pub fn with_capacity(reader: R, deframer: D, capacity: usize) -> Self {
        Self {
            reader: CountingReader::new(reader),
            deframer,
            buffer: Vec::with_capacity(capacity),
            memory: NoMemoryPolicy,
            start_offset: 0,
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
            start_offset: self.start_offset,
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
        self.memory.before_read(&mut self.buffer);
        let frame_start = self.bytes_consumed();
        match self
            .deframer
            .read_and_deframe(&mut self.reader, &mut self.buffer)?
        {
            Some(n) => {
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
            None => Ok(None),
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

    /// Number of source bytes consumed, plus the configured start offset.
    ///
    /// After a successful frame this is the offset where the next frame begins.
    /// If a read fails mid-frame, it reflects bytes already consumed by that
    /// failed attempt.
    pub fn bytes_consumed(&self) -> u64 {
        self.start_offset + self.reader.count
    }

    /// Sets the base used by [`bytes_consumed`](Self::bytes_consumed) and
    /// receipts returned by [`read_message_with_receipt`](Self::read_message_with_receipt).
    ///
    /// Set this before reading when the wrapped source begins at a nonzero
    /// stream position.
    #[must_use]
    pub fn with_start_offset(mut self, offset: u64) -> Self {
        self.start_offset = offset;
        self
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
        let mut memory = ReaderMemoryPolicy {
            policy: crate::policy::NoOpPolicy,
            baseline_capacity: 64,
            pending_shrink: true,
        };

        memory.before_read(&mut buffer);

        assert!(buffer.is_empty(), "old payload bytes are no longer exposed");
        assert!(
            buffer.capacity() >= 64 && buffer.capacity() <= original_capacity,
            "shrink_to keeps at least the baseline without growing capacity"
        );
        assert!(!memory.pending_shrink);

        let reclaimed_capacity = buffer.capacity();
        memory.before_read(&mut buffer);
        assert_eq!(
            buffer.capacity(),
            reclaimed_capacity,
            "a completed reclamation is not repeated"
        );
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
