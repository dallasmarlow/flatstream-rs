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

/// A trait for types that can be deserialized from a FlatBuffer message payload.
///
/// This trait provides a type-safe and ergonomic way to access the root
/// of a FlatBuffer message from a raw byte slice (`&[u8]`). The primary
/// goal is to improve the reading path without introducing any performance
/// overhead, fully preserving the library's zero-copy principles.
///
/// Implementations of this trait are responsible for calling `flatbuffers::get_root`
/// and handling any FlatBuffer-specific verification.
pub trait StreamDeserialize<'a>: Sized {
    /// The associated type `Root` will be the generated FlatBuffer accessor struct
    /// (e.g., `MyEvent<'a>`). This type must implement `flatbuffers::Follow<'a>`
    /// to allow safe access to the FlatBuffer data.
    type Root: flatbuffers::Follow<'a>;

    /// Safely accesses the FlatBuffer root from a given payload slice.
    ///
    /// This method is responsible for performing the FlatBuffer verification
    /// and returning the strongly-typed root accessor object. It should leverage
    /// `flatbuffers::get_root` internally.
    ///
    /// # Arguments
    /// * `payload` - A borrowed slice of bytes containing the FlatBuffer message.
    ///
    /// # Returns
    /// * `Ok(Self::Root)` - The successfully accessed FlatBuffer root object.
    /// * `Err(flatstream::Error::FlatbuffersError)` - If the FlatBuffer is invalid.
    fn from_payload(payload: &'a [u8]) -> Result<Self::Root>;
}
