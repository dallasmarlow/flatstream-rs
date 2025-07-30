//! A generic, composable reader for `flatstream`.

use crate::error::Result;
use crate::framing::Deframer;
use std::io::Read;
use std::marker::PhantomData;

/// A reader for streaming messages from a `flatstream`.
///
/// This reader is generic over a `Deframer` strategy, which defines how
/// each message is parsed from the byte stream. It provides two APIs:
///
/// 1. **Processor API** (`process_all()`): High-performance closure-based processing
/// 2. **Expert API** (`messages()`): Manual iteration for maximum control
///
/// # Performance: Processor API vs. Expert API
///
/// The `process_all()` method provides the highest performance by using a closure
/// that receives borrowed slices (`&[u8]`) directly from the internal buffer. This
/// eliminates all allocations and provides zero-copy access to message payloads.
///
/// The `messages()` method provides manual iteration control for cases where you
/// need more complex control flow or want to process messages conditionally.
///
/// ```rust
/// # use flatstream_rs::{StreamReader, DefaultDeframer, Result};
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
pub struct StreamReader<R: Read, D: Deframer> {
    reader: R,
    deframer: D,
    // The reader owns its buffer, resizing as needed.
    // This addresses Lesson 4 and 16 for memory efficiency.
    buffer: Vec<u8>,
    _phantom: PhantomData<D>, // PhantomData because D is only used in method calls
}

impl<R: Read, D: Deframer> StreamReader<R, D> {
    /// Creates a new `StreamReader` with the given reader and deframing strategy.
    pub fn new(reader: R, deframer: D) -> Self {
        Self {
            reader,
            deframer,
            buffer: Vec::new(),
            _phantom: PhantomData,
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
        loop {
            match self.read_message()? {
                Some(payload) => {
                    // Process the payload using the user's closure
                    processor(payload)?;
                }
                None => {
                    // Clean end of stream
                    break;
                }
            }
        }
        Ok(())
    }

    /// Returns an iterator-like object for manual message processing.
    ///
    /// This provides the "expert path" for users who need more control over
    /// the iteration process. Each call to `next()` returns a borrowed slice
    /// to the message payload, providing zero-copy access.
    pub fn messages(&mut self) -> Messages<'_, R, D> {
        Messages { reader: self }
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
    pub fn next(&mut self) -> Result<Option<&[u8]>> {
        self.reader.read_message()
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
    fn test_read_message_with_checksum() {
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
    fn test_read_message_with_checksum_feature() {
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
    fn test_read_message_without_checksum() {
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
            let data = builder.create_string(&format!("message {}", i));
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
            let data = builder.create_string(&format!("message {}", i));
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
            let data = builder.create_string(&format!("message {}", i));
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
            let data = builder.create_string(&format!("message {}", i));
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
                return Err(crate::error::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "Simulated processing error"
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
            e => panic!("Expected ChecksumMismatch error, got: {:?}", e),
        }
    }
}
