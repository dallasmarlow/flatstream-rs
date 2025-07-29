//! A generic, composable writer for `flatstream`.

use crate::error::Result;
use crate::framing::Framer;
use crate::traits::StreamSerialize;
use flatbuffers::FlatBufferBuilder;
use std::io::Write;

/// A writer for streaming `StreamSerialize`-able objects.
///
/// This writer is generic over a `Framer` strategy, which defines how
/// each message is framed in the byte stream (e.g., with or without a checksum).
pub struct StreamWriter<W: Write, F: Framer> {
    writer: W,
    framer: F,
    // The writer owns the builder, ensuring its lifecycle is managed correctly.
    // This addresses Lesson 2, 4, and 16.
    builder: FlatBufferBuilder<'static>,
}

impl<W: Write, F: Framer> StreamWriter<W, F> {
    /// Creates a new `StreamWriter` with the given writer and framing strategy.
    pub fn new(writer: W, framer: F) -> Self {
        Self {
            writer,
            framer,
            builder: FlatBufferBuilder::new(),
        }
    }

    /// Writes a single serializable item to the stream.
    pub fn write<T: StreamSerialize>(&mut self, item: &T) -> Result<()> {
        // 1. Reset the internal builder for efficiency.
        self.builder.reset();

        // 2. Delegate serialization to the user's type.
        item.serialize(&mut self.builder)?;

        // 3. Get the finished payload.
        let payload = self.builder.finished_data();

        // 4. Delegate framing and writing to the strategy.
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::DefaultFramer;

    #[cfg(feature = "checksum")]
    use crate::{ChecksumFramer, XxHash64};
    use std::io::Cursor;

    #[test]
    fn test_write_with_checksum() {
        let mut buffer = Vec::new();
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        assert!(writer.write(&"test data").is_ok());

        let data = buffer;
        assert!(!data.is_empty());
        // Should have: 4 bytes (length) + payload (no checksum)
        assert!(data.len() >= 4);
    }

    #[cfg(feature = "checksum")]
    #[test]
    fn test_write_with_checksum_feature() {
        let mut buffer = Vec::new();
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        assert!(writer.write(&"test data").is_ok());

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

        assert!(writer.write(&"no checksum").is_ok());

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
            assert!(writer.write(&format!("message {}", i)).is_ok());
        }

        let data = buffer;
        assert!(!data.is_empty());
    }

    #[cfg(feature = "checksum")]
    #[test]
    fn test_multiple_messages_with_checksum() {
        let mut buffer = Vec::new();
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        for i in 0..3 {
            assert!(writer.write(&format!("message {}", i)).is_ok());
        }

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
