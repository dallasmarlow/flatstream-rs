//! Core traits for the flatstream library.

use crate::error::Result;
use flatbuffers::FlatBufferBuilder;

/// A trait for types that can be serialized into a `flatstream`.
///
/// This trait provides a method to serialize data into a FlatBuffer.
/// The implementation should handle the complete serialization process
/// including calling finish() on the builder.
pub trait StreamSerialize {
    /// Serializes the object into the provided FlatBuffer builder.
    /// The implementation should reset the builder, serialize the data,
    /// and call finish() on the builder.
    fn serialize_to(&self, builder: &mut FlatBufferBuilder) -> Result<()>;
}

// For simple types like strings, we provide a direct implementation
impl StreamSerialize for &str {
    fn serialize_to(&self, builder: &mut FlatBufferBuilder) -> Result<()> {
        builder.reset();

        // For strings, we need to work around the borrowing issue
        // by creating the data as a byte vector
        let bytes = self.as_bytes();
        builder.start_vector::<u8>(bytes.len() + 1);

        // Write bytes in reverse order (FlatBuffers builds backwards)
        builder.push(0u8); // null terminator
        for &byte in bytes.iter().rev() {
            builder.push(byte);
        }

        let vec_offset = builder.end_vector::<u8>(bytes.len() + 1);
        builder.finish_minimal(vec_offset);

        Ok(())
    }
}

// Also implement for owned strings for convenience
impl StreamSerialize for String {
    fn serialize_to(&self, builder: &mut FlatBufferBuilder) -> Result<()> {
        self.as_str().serialize_to(builder)
    }
}
