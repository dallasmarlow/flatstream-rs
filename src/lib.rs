//! # FlatStream (v0.2.7)
//!
//! A lightweight, composable, high-performance Rust library for streaming FlatBuffers.
//!
//! ## Overview
//!
//! `flatstream-rs` provides a flexible, trait-based architecture for efficiently streaming
//! FlatBuffers messages with optional data integrity checks. The library separates concerns
//! through composable traits, making it easy to customize framing strategies and serialization
//! behavior.
//!
//! ## Key Features
//!
//! * **Composable Architecture**: Separate traits for serialization, framing, and checksums
//! * **Flexible Framing**: Choose between simple length-prefixed or checksum-protected framing
//! * **Borrowed Payload Access**: Direct `&[u8]` access through the Processor API
//! * **Memory Efficient**: Reusable buffers and minimal allocations
//! * **Type Safe**: Generic over I/O types and framing strategies
//!
//! ## Quick Start
//!
//! ```rust
//! use flatstream::*;
//! use flatbuffers::FlatBufferBuilder;
//! use std::io::Cursor;
//!
//! // Define your serializable type
//! struct MyData {
//!     message: String,
//!     value: i32,
//! }
//!
//! impl StreamSerialize for MyData {
//!     fn serialize<A: flatbuffers::Allocator>(&self, builder: &mut FlatBufferBuilder<A>) -> Result<()> {
//!         let message = builder.create_string(&self.message);
//!         // Build your FlatBuffer here...
//!         builder.finish(message, None);
//!         Ok(())
//!     }
//! }
//!
//! fn main() -> Result<()> {
//!     // Write with default framing. Any `Write` works — swap the Cursor for
//!     // `BufWriter::new(File::create("data.bin")?)` to journal to disk.
//!     let mut storage = Vec::new();
//!     let framer = DefaultFramer;
//!     let mut writer = StreamWriter::new(Cursor::new(&mut storage), framer);
//!
//!     let data = MyData { message: "Hello".to_string(), value: 42 };
//!
//!     // Write the data directly (simple mode)
//!     writer.write(&data)?;
//!     writer.flush()?;
//!
//!     // Read with default deframing using the Processor API
//!     let deframer = DefaultDeframer::new();
//!     let mut reader = StreamReader::new(Cursor::new(&storage), deframer);
//!
//!     // Note: `payload` is valid only until the next successful read.
//!     let mut count = 0;
//!     reader.process_all(|payload| {
//!         assert!(!payload.is_empty());
//!         count += 1;
//!         Ok(())
//!     })?;
//!     assert_eq!(count, 1);
//!
//!     Ok(())
//! }
//! ```
//!
//! ## Architecture
//!
//! The library is built around three core traits:
//!
//! * **`StreamSerialize`**: User types implement this to define how they serialize to FlatBuffers
//! * **`Framer`**: Defines how messages are framed in the byte stream (e.g., with/without checksums)
//! * **`Deframer`**: Defines how messages are parsed from the byte stream
//!
//! This separation allows for maximum flexibility and composability.
//!
//! ## Validation (Zero-Copy)
//!
//! Validation is an optional, composable layer that operates directly on the
//! in-place payload slice (`&[u8]`). On successful validation it adds no payload
//! copy or allocation; failures allocate their diagnostic reason.
//!
//! - Writers: `FramerExt::with_validator(..)` validates before bytes are written.
//! - Readers: `DeframerExt::with_validator(..)` validates after deframing (and
//!   after checksum verification if present) but before yielding to user code.
//! - Opt-out: `NoValidator` is a zero-cost path (`#[inline(always)]`) and is
//!   optimized away by the compiler in release builds.
//!
//! ```rust
//! # use flatstream::*;
//! # use std::io::Cursor;
//! // Read with structural validation (type-agnostic)
//! let data: Vec<u8> = vec![]; // framed bytes
//! let deframer = DefaultDeframer::new().with_validator(TableRootValidator::new());
//! let mut reader = StreamReader::new(Cursor::new(data), deframer);
//! reader.process_all(|payload| {
//!     // payload is an in-place &[u8] slice; validation adds no copies, and
//!     // no allocations on the success path (a failure allocates its reason)
//!     Ok(())
//! })?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

// The default build is provably free of `unsafe`: the only unsafe block in the
// crate is the opt-in `unsafe_typed` verification-skipping path (reader.rs).
#![cfg_attr(not(feature = "unsafe_typed"), forbid(unsafe_code))]

pub mod checksum;
pub mod error;
pub mod framing;
pub mod policy;
pub mod reader;
pub mod traits;
pub mod validation;
pub mod writer;

// Re-export the main public API for user convenience.
pub use checksum::NoChecksum;
pub use error::{Error, ErrorKind, Result};
pub use framing::{
    BoundedFramer, DefaultDeframer, DefaultFramer, Deframer, DeframerExt, Framer, FramerExt,
    ValidatingDeframer, ValidatingFramer, DEFAULT_MAX_FRAME_LEN,
};
pub use policy::{
    AdaptiveWatermarkPolicy, Clock, MemoryPolicy, MonotonicClock, NoOpPolicy, ReclamationInfo,
    ReclamationReason, SizeThresholdPolicy,
};
pub use reader::{Messages, StreamReader, TypedMessages};
pub use traits::StreamDeserialize;
pub use traits::StreamSerialize;
pub use validation::{
    CompositeValidator, NoValidator, SizeValidator, TableRootValidator, TypedValidator, Validator,
};
pub use writer::StreamWriter;

#[cfg(feature = "xxhash")]
pub use checksum::XxHash64;
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
pub use framing::{ChecksumDeframer, ChecksumFramer};

#[cfg(feature = "crc32")]
pub use checksum::Crc32;

#[cfg(feature = "crc16")]
pub use checksum::Crc16;
