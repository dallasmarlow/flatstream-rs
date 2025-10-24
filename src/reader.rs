//! A generic, composable reader for `flatstream`.

use crate::error::Result;
use crate::framing::Deframer;
use crate::traits::StreamDeserialize;
use std::io::Read;
use std::marker::PhantomData;

/// A reader for streaming messages from a `flatstream`.
///
/// This reader is generic over a `Deframer` strategy, which defines how
/// each message is parsed from the byte stream.
///
/// **Zero-Copy Guarantee**: Both APIs provide direct access to the internal buffer
/// as `&[u8]` slices. The FlatBuffers philosophy is preserved - no parsing, no
/// copying, just direct access to the serialized data.
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
/// The `process_all()` method provides the highest performance by using a closure
/// that receives borrowed slices (`&[u8]`) directly from the internal buffer. This
/// eliminates all allocations and provides zero-copy access to message payloads.
/// Performance: Excellent - zero-copy access to message payloads.
///
/// The `messages()` method provides manual iteration control for cases where you
/// need more complex control flow or want to process messages conditionally.
/// Performance: Same as `process_all()` - both use zero-copy access.
///
/// ```rust
/// # use flatstream::{StreamReader, DefaultDeframer, Result};
/// # use std::io::Cursor;
/// # let mut reader = StreamReader::new(Cursor::new(vec![]), DefaultDeframer);
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
/// ## Choosing a Deframer
///
/// This reader is generic over a `Deframer` strategy. You can choose from several built-in implementations based on your performance and safety needs:
///
/// * **`DefaultDeframer` (Recommended)**: The standard, safe implementation. It reads the exact number of bytes specified by the length prefix. It is safe and performs well for almost all use cases.
///
/// * **`SafeTakeDeframer`**: An alternative safe implementation that uses `Read::take`. Its performance may vary depending on the underlying reader, but it provides another safe option.
///
/// * **`UnsafeDeframer` (Expert)**: The highest-performance option, intended for scenarios where you have a trusted data source (e.g., reading a file you just wrote). It avoids initializing the buffer by using `unsafe` code, which can provide a speed boost by eliminating writes to memory. **Only use this if you have benchmarked it and understand the risks.**
///
pub struct StreamReader<R: Read, D: Deframer> {
    reader: R,
    deframer: D,
    // The reader owns its buffer, resizing as needed.
    // This addresses Lesson 4 and 16 for memory efficiency.
    buffer: Vec<u8>,
}

impl<R: Read, D: Deframer> StreamReader<R, D> {
    /// Creates a new `StreamReader` with the given reader and deframing strategy.
    pub fn new(reader: R, deframer: D) -> Self {
        Self {
            reader,
            deframer,
            buffer: Vec::new(),
        }
    }

    /// Creates a new `StreamReader` with a pre-allocated buffer capacity.
    pub fn with_capacity(reader: R, deframer: D, capacity: usize) -> Self {
        Self {
            reader,
            deframer,
            buffer: Vec::with_capacity(capacity),
        }
    }

    /// Reads the next message into the internal buffer. This is the low-level
    /// alternative to using the processor or expert APIs.
    /// Returns Ok(Some(payload)) on success, Ok(None) on clean EOF.
    pub fn read_message(&mut self) -> Result<Option<&[u8]>> {
        match self
            .deframer
            .read_and_deframe(&mut self.reader, &mut self.buffer)?
        {
            Some(_) => Ok(Some(&self.buffer)),
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
    ///         flatbuffers::root::<&'a str>(payload).map_err(Error::FlatbuffersError)
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
    /// let mut reader = StreamReader::new(Cursor::new(&buf), DefaultDeframer);
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
    /// Safety: Only use when the payloads are guaranteed to be valid for the
    /// expected `T::Root`. This skips FlatBuffers verification and relies on trusted data.
    #[cfg(feature = "unsafe_typed")]
    pub fn process_typed_unchecked<T, F>(&mut self, mut processor: F) -> Result<()>
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
    pub fn next_message(&mut self) -> Result<Option<&[u8]>> {
        self.reader.read_message()
    }

    #[allow(clippy::should_implement_trait)]
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
    ///         flatbuffers::root::<&'a str>(payload).map_err(Error::FlatbuffersError)
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
    /// let mut r = StreamReader::new(Cursor::new(&buf), DefaultDeframer);
    /// let mut it = r.typed_messages::<StrRoot>();
    /// let first = it.next().unwrap().unwrap();
    /// assert_eq!(first, "hello");
    /// # Ok(()) }
    /// ```
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

    #[test]
    fn test_read_message_default_framer() {
        // Write a message first
        let mut buffer = Vec::new();
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("test data");
        builder.finish(data, None);
        writer.write_finished(&mut builder).unwrap();

        // Now read it back
        let data = buffer;
        let deframer = DefaultDeframer;
        let mut reader = StreamReader::new(Cursor::new(data), deframer);

        let result = reader.read_message().unwrap();
        assert!(result.is_some());
        let payload = result.unwrap();
        assert!(!payload.is_empty());
    }

    #[cfg(feature = "xxhash")]
    #[test]
    fn test_read_message_with_xxhash64_feature() {
        // Write a message first
        let mut buffer = Vec::new();
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("test data");
        builder.finish(data, None);
        writer.write_finished(&mut builder).unwrap();

        // Now read it back
        let data = buffer;
        let checksum = XxHash64::new();
        let deframer = ChecksumDeframer::new(checksum);
        let mut reader = StreamReader::new(Cursor::new(data), deframer);

        let result = reader.read_message().unwrap();
        assert!(result.is_some());
        let payload = result.unwrap();
        assert!(!payload.is_empty());
    }

    #[test]
    fn test_read_message_default_no_checksum() {
        // Write a message first
        let mut buffer = Vec::new();
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("no checksum");
        builder.finish(data, None);
        writer.write_finished(&mut builder).unwrap();

        // Now read it back
        let data = buffer;
        let deframer = DefaultDeframer;
        let mut reader = StreamReader::new(Cursor::new(data), deframer);

        let result = reader.read_message().unwrap();
        assert!(result.is_some());
        let payload = result.unwrap();
        assert!(!payload.is_empty());
    }

    #[test]
    fn test_process_all() {
        // Write multiple messages
        let mut buffer = Vec::new();
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        for i in 0..3 {
            let mut builder = FlatBufferBuilder::new();
            let data = builder.create_string(&format!("message {i}"));
            builder.finish(data, None);
            writer.write_finished(&mut builder).unwrap();
        }

        // Read them back using process_all
        let data = buffer;
        let deframer = DefaultDeframer;
        let mut reader = StreamReader::new(Cursor::new(data), deframer);

        let mut count = 0;
        reader
            .process_all(|payload| {
                assert!(!payload.is_empty());
                count += 1;
                Ok(())
            })
            .unwrap();

        assert_eq!(count, 3);
    }

    #[test]
    fn test_messages_expert_api() {
        // Write multiple messages
        let mut buffer = Vec::new();
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        for i in 0..3 {
            let mut builder = FlatBufferBuilder::new();
            let data = builder.create_string(&format!("message {i}"));
            builder.finish(data, None);
            writer.write_finished(&mut builder).unwrap();
        }

        // Read them back using the expert API
        let data = buffer;
        let deframer = DefaultDeframer;
        let mut reader = StreamReader::new(Cursor::new(data), deframer);

        let mut count = 0;
        let mut messages = reader.messages();
        while let Some(payload) = messages.next().unwrap() {
            assert!(!payload.is_empty());
            count += 1;
        }
        assert_eq!(count, 3);
    }

    #[cfg(feature = "xxhash")]
    #[test]
    fn test_process_all_with_checksum() {
        // Write multiple messages
        let mut buffer = Vec::new();
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        for i in 0..3 {
            let mut builder = FlatBufferBuilder::new();
            let data = builder.create_string(&format!("message {i}"));
            builder.finish(data, None);
            writer.write_finished(&mut builder).unwrap();
        }

        // Read them back using process_all
        let data = buffer;
        let checksum = XxHash64::new();
        let deframer = ChecksumDeframer::new(checksum);
        let mut reader = StreamReader::new(Cursor::new(data), deframer);

        let mut count = 0;
        reader
            .process_all(|payload| {
                assert!(!payload.is_empty());
                count += 1;
                Ok(())
            })
            .unwrap();

        assert_eq!(count, 3);
    }

    #[test]
    fn test_read_empty_stream() {
        let empty_data = Vec::new();
        let deframer = DefaultDeframer;
        let mut reader = StreamReader::new(Cursor::new(empty_data), deframer);

        let result = reader.read_message().unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_process_all_empty_stream() {
        let empty_data = Vec::new();
        let deframer = DefaultDeframer;
        let mut reader = StreamReader::new(Cursor::new(empty_data), deframer);

        let mut count = 0;
        reader
            .process_all(|_payload| {
                count += 1;
                Ok(())
            })
            .unwrap();

        assert_eq!(count, 0);
    }

    #[test]
    fn test_process_all_error_propagation() {
        // Write some test data first
        let mut buffer = Vec::new();
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        let mut builder = FlatBufferBuilder::new();
        for i in 0..5 {
            builder.reset();
            let data = builder.create_string(&format!("message {i}"));
            builder.finish(data, None);
            writer.write_finished(&mut builder).unwrap();
        }

        // Now test error propagation in process_all
        let data = buffer;
        let deframer = DefaultDeframer;
        let mut reader = StreamReader::new(Cursor::new(data), deframer);

        let mut count = 0;
        let result = reader.process_all(|_payload| {
            count += 1;

            // Simulate an error on the third message
            if count == 3 {
                return Err(crate::error::Error::Io(std::io::Error::other(
                    "Simulated processing error",
                )));
            }

            Ok(())
        });

        // Should get an error
        assert!(result.is_err());

        // Should have processed exactly 3 messages before the error
        assert_eq!(count, 3);

        // Verify the error is the one we created
        match result.unwrap_err() {
            crate::error::Error::Io(e) => {
                assert_eq!(e.kind(), std::io::ErrorKind::Other);
                assert_eq!(e.to_string(), "Simulated processing error");
            }
            _ => panic!("Expected Io error"),
        }
    }

    #[cfg(feature = "xxhash")]
    #[test]
    fn test_checksum_mismatch() {
        // Write a message with checksum
        let mut buffer = Vec::new();
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        let mut builder = FlatBufferBuilder::new();
        let data = builder.create_string("test data");
        builder.finish(data, None);
        writer.write_finished(&mut builder).unwrap();

        // Corrupt the data by flipping a bit
        let mut data = buffer;
        if data.len() > 20 {
            data[20] ^= 1; // Flip a bit in the payload
        }

        // Try to read the corrupted data
        let checksum = XxHash64::new();
        let deframer = ChecksumDeframer::new(checksum);
        let mut reader = StreamReader::new(Cursor::new(data), deframer);
        let result = reader.read_message();

        // Should get a checksum mismatch error
        assert!(result.is_err());
        match result.unwrap_err() {
            crate::error::Error::ChecksumMismatch { .. } => {} // Expected
            e => panic!("Expected ChecksumMismatch error, got: {e:?}"),
        }
    }
}
