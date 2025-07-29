//! A generic, composable reader for `flatstream`.

use crate::error::Result;
use crate::framing::Deframer;
use std::io::Read;
use std::marker::PhantomData;

/// A reader for streaming messages from a `flatstream`.
///
/// This reader is generic over a `Deframer` strategy, which defines how
/// each message is parsed from the byte stream. It implements `Iterator`
/// to provide an ergonomic way to process messages.
///
/// # Performance: Iterator vs. `read_message()`
///
/// This struct implements the `Iterator` trait for ergonomic use in `for` loops.
/// The `next()` method returns a `Result<Vec<u8>>`, which involves cloning the
/// message payload from the internal buffer into a new `Vec`. This is safe and
/// convenient but involves a heap allocation per message.
///
/// For performance-critical paths where allocations must be minimized, prefer
/// using the `read_message()` method directly in a `while let` loop. This method
/// returns a `Result<Option<&[u8]>>`, which is a zero-copy borrow of the
/// reader's internal buffer.
///
/// ```rust
/// # use flatstream_rs::{StreamReader, DefaultDeframer, Result};
/// # use std::io::Cursor;
/// # let mut reader = StreamReader::new(Cursor::new(vec![]), DefaultDeframer);
/// // High-performance, zero-allocation read loop
/// while let Some(payload_slice) = reader.read_message()? {
///     // Process the payload_slice directly
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
    /// alternative to using the iterator interface.
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
}

impl<R: Read, D: Deframer> Iterator for StreamReader<R, D> {
    type Item = Result<Vec<u8>>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.read_message() {
            Ok(Some(payload)) => Some(Ok(payload.to_vec())), // Return a copy for iterator safety
            Ok(None) => None,                                // Clean end of stream
            Err(e) => Some(Err(e)),                          // An error occurred
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::DefaultDeframer;
    use crate::framing::DefaultFramer;
    use crate::writer::StreamWriter;

    #[cfg(feature = "xxhash")]
    use crate::{ChecksumDeframer, ChecksumFramer, XxHash64};
    use std::io::Cursor;

    #[test]
    fn test_read_message_with_checksum() {
        // Write a message first
        let mut buffer = Vec::new();
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        writer.write(&"test data").unwrap();

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
        writer.write(&"test data").unwrap();

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
        writer.write(&"no checksum").unwrap();

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
    fn test_read_multiple_messages() {
        // Write multiple messages
        let mut buffer = Vec::new();
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        for i in 0..3 {
            writer.write(&format!("message {}", i)).unwrap();
        }

        // Read them back
        let data = buffer;
        let deframer = DefaultDeframer;
        let mut reader = StreamReader::new(Cursor::new(data), deframer);

        let mut count = 0;
        while let Some(result) = reader.next() {
            assert!(result.is_ok());
            count += 1;
        }
        assert_eq!(count, 3);
    }

    #[cfg(feature = "xxhash")]
    #[test]
    fn test_read_multiple_messages_with_checksum() {
        // Write multiple messages
        let mut buffer = Vec::new();
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        for i in 0..3 {
            writer.write(&format!("message {}", i)).unwrap();
        }

        // Read them back
        let data = buffer;
        let checksum = XxHash64::new();
        let deframer = ChecksumDeframer::new(checksum);
        let mut reader = StreamReader::new(Cursor::new(data), deframer);

        let mut count = 0;
        while let Some(result) = reader.next() {
            assert!(result.is_ok());
            count += 1;
        }
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

    #[cfg(feature = "xxhash")]
    #[test]
    fn test_checksum_mismatch() {
        // Write a message with checksum
        let mut buffer = Vec::new();
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        writer.write(&"test data").unwrap();

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
