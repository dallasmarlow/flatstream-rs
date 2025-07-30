//! Core traits for the flatstream library.

use crate::error::Result;
use flatbuffers::FlatBufferBuilder;

/// A trait for types that can be serialized into a `flatstream`.
///
/// By implementing this trait for your types, you provide the logic for
/// how they should be written into a FlatBuffers message. The library's
/// `StreamWriter` will then handle the framing and I/O.
///
/// The trait is generic over the allocator type to ensure zero-copy,
/// high-performance serialization without any temporary allocations or data copying.
pub trait StreamSerialize {
    /// Serializes the object using the provided FlatBuffer builder.
    ///
    /// The implementation of this method is responsible for building the
    /// FlatBuffer message and calling `builder.finish()` or a related
    /// method to finalize the buffer for writing.
    ///
    /// # Arguments
    /// * `builder` - A mutable reference to a `FlatBufferBuilder` with any allocator type.
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()>;
}

// A simple implementation for strings to facilitate testing and examples.
// This addresses Lesson 5 and 9 by starting with simple, reliable data.
impl StreamSerialize for &str {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let data = builder.create_string(self);
        builder.finish(data, None);
        Ok(())
    }
}

// Also implement for owned strings for convenience
impl StreamSerialize for String {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        self.as_str().serialize(builder)
    }
}
