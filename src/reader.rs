//! A generic, composable reader for `flatstream`.

use crate::error::Result;
use crate::framing::Deframer;
use crate::policy::{MemoryPolicy, ReclamationInfo};
use crate::traits::StreamDeserialize;
use std::io::Read;
use std::marker::PhantomData;

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
/// steady-state reads perform no allocation and no per-frame zeroing. The
/// deframers bound each frame's declared length (default
/// [`DEFAULT_MAX_FRAME_LEN`](crate::framing::DEFAULT_MAX_FRAME_LEN), 16 MiB)
/// *before* sizing any allocation from it; unbounded reading is an explicit
/// opt-in (e.g. `DefaultDeframer::unbounded()`) for fully trusted streams.
///
/// ## Memory Reclamation
///
/// The internal buffer grows to the largest message seen and keeps that
/// capacity. For long-running processes with bursty workloads, an optional
/// [`MemoryPolicy`] can be installed with
/// [`with_memory_policy`](Self::with_memory_policy) to shrink the buffer back
/// to the policy's baseline capacity (`MemoryPolicy::baseline_capacity`,
/// default 16 KiB). The shrink is deferred to the start of the next read, so a
/// payload already returned is never invalidated.
pub struct StreamReader<R: Read, D: Deframer> {
    reader: R,
    deframer: D,
    // The reader owns its buffer, resizing as needed.
    // This addresses Lesson 4 and 16 for memory efficiency.
    buffer: Vec<u8>,
    // Optional capacity-aware policy; one predictable branch per read when absent.
    policy: Option<PolicySlot>,
    pending_shrink: bool,
}

/// Installed-policy state: the policy plus its baseline (cached from
/// `MemoryPolicy::baseline_capacity()` at installation so the steady-state gate
/// is a plain integer compare).
struct PolicySlot {
    policy: Box<dyn MemoryPolicy>,
    baseline_capacity: usize,
}

impl<R: Read, D: Deframer> StreamReader<R, D> {
    /// Creates a new `StreamReader` with the given reader and deframing strategy.
    pub fn new(reader: R, deframer: D) -> Self {
        Self {
            reader,
            deframer,
            buffer: Vec::new(),
            policy: None,
            pending_shrink: false,
        }
    }

    /// Creates a new `StreamReader` with a pre-allocated buffer capacity.
    pub fn with_capacity(reader: R, deframer: D, capacity: usize) -> Self {
        Self {
            reader,
            deframer,
            buffer: Vec::with_capacity(capacity),
            policy: None,
            pending_shrink: false,
        }
    }

    /// Installs a memory reclamation policy on this reader.
    ///
    /// After each successful read, the policy observes the payload size and the
    /// internal buffer's capacity; when it fires, the buffer is replaced with a
    /// fresh one at the policy's baseline capacity
    /// (`MemoryPolicy::baseline_capacity`, cached here at installation) —
    /// deferred to the start of the *next* read so the payload just returned is
    /// never invalidated. The policy is consulted only while the buffer's
    /// capacity exceeds that baseline — at or below it there is nothing to
    /// reclaim.
    pub fn with_memory_policy<P: MemoryPolicy + 'static>(mut self, policy: P) -> Self {
        self.policy = Some(PolicySlot {
            baseline_capacity: policy.baseline_capacity(),
            policy: Box::new(policy),
        });
        self
    }

    /// Reads the next message into the internal buffer. This is the low-level
    /// alternative to using the processor or expert APIs.
    /// Returns Ok(Some(payload)) on success, Ok(None) on clean EOF.
    ///
    /// The policy machinery is outlined into cold/uninlined helpers so this
    /// hot path stays small enough to inline; without a policy installed the
    /// per-read cost is two predictable, never-taken branches.
    #[inline]
    pub fn read_message(&mut self) -> Result<Option<&[u8]>> {
        // If a shrink was scheduled on a previous frame, perform it now
        if self.pending_shrink {
            self.apply_pending_shrink();
        }
        match self
            .deframer
            .read_and_deframe(&mut self.reader, &mut self.buffer)?
        {
            Some(n) => {
                if self.policy.is_some() {
                    self.evaluate_memory_policy(n);
                }
                Ok(Some(&self.buffer[..n]))
            }
            None => Ok(None),
        }
    }

    /// Applies a reclaim scheduled by the previous read. Cold: runs at most
    /// once per reclamation event, never on the steady-state path.
    #[cold]
    #[inline(never)]
    fn apply_pending_shrink(&mut self) {
        // `pending_shrink` is only ever set by an installed policy.
        if let Some(slot) = self.policy.as_ref() {
            self.buffer = Vec::with_capacity(slot.baseline_capacity);
        }
        self.pending_shrink = false;
    }

    /// Consults the installed policy after a successful read. Outlined
    /// (`inline(never)`) to keep `read_message`'s inlinable body minimal for
    /// readers without a policy.
    #[inline(never)]
    fn evaluate_memory_policy(&mut self, last_message_size: usize) {
        let Some(slot) = self.policy.as_mut() else {
            return;
        };
        let capacity = self.buffer.capacity();
        // At or below the policy's baseline there is nothing to reclaim —
        // skip the policy so its state cannot churn.
        if capacity > slot.baseline_capacity {
            if let Some(reason) = slot.policy.should_reset(last_message_size, capacity) {
                // Schedule the shrink for the start of the *next* read so the
                // payload about to be returned is never invalidated.
                self.pending_shrink = true;
                slot.policy.on_reclaim(&ReclamationInfo {
                    reason,
                    last_message_size,
                    capacity_before: capacity,
                    capacity_after: slot.baseline_capacity,
                });
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

    /// Returns an iterator-like object for manual message processing.
    ///
    /// This provides the "expert path" for users who need more control over
    /// the iteration process. Each call to `next_message()` returns a borrowed slice
    /// to the message payload, providing zero-copy access.
    ///
    /// Lifetimes: Each returned payload `&[u8]` is valid only until the next successful read.
    pub fn messages(&mut self) -> Messages<'_, R, D> {
        Messages { reader: self }
    }

    /// Returns a typed iterator-like object for manual message processing.
    ///
    /// This yields verified FlatBuffer roots using the `StreamDeserialize` trait
    /// while preserving zero-copy lifetimes tied to the reader.
    pub fn typed_messages<T>(&mut self) -> TypedMessages<'_, R, D, T>
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

    /// Returns a reference to the underlying reader.
    pub fn get_ref(&self) -> &R {
        &self.reader
    }

    /// Returns a mutable reference to the underlying reader.
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.reader
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

    /// Consume the `StreamReader`, returning the underlying reader.
    pub fn into_inner(self) -> R {
        self.reader
    }
}

/// An iterator-like object for manual message processing.
///
/// This struct provides the "expert path" for users who need more control over
/// the iteration process. It borrows the `StreamReader` mutably, ensuring
/// proper lifetime management.
pub struct Messages<'a, R: Read, D: Deframer> {
    reader: &'a mut StreamReader<R, D>,
}

impl<'a, R: Read, D: Deframer> Messages<'a, R, D> {
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

    #[allow(clippy::should_implement_trait)]
    #[inline]
    pub fn next(&mut self) -> Result<Option<&[u8]>> {
        self.next_message()
    }
}

/// Typed iterator-like object yielding verified FlatBuffer roots.
pub struct TypedMessages<'a, R: Read, D: Deframer, T>
where
    for<'p> T: StreamDeserialize<'p>,
{
    reader: &'a mut StreamReader<R, D>,
    _phantom: PhantomData<T>,
}

impl<'a, R: Read, D: Deframer, T> TypedMessages<'a, R, D, T>
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
