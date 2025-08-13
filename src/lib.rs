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
//! * **Zero-Copy Reading**: Direct access to FlatBuffer payloads through the Processor API
//! * **Memory Efficient**: Reusable buffers and minimal allocations
//! * **Type Safe**: Generic over I/O types and framing strategies
//!
//! ## Quick Start
//!
//! ```rust
//! use flatstream::*;
//! use std::fs::File;
//! use flatbuffers::FlatBufferBuilder;
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
//!     // Write with default framing
//!     let file = File::create("data.bin")?;
//!     let framer = DefaultFramer;
//!     let mut writer = StreamWriter::new(file, framer);
//!
//!     let data = MyData { message: "Hello".to_string(), value: 42 };
//!     
//!     // Write the data directly (simple mode)
//!     writer.write(&data)?;
//!     writer.flush()?;
//!
//!     // Read with default deframing using the Processor API
//!     let file = File::open("data.bin")?;
//!     let deframer = DefaultDeframer;
//!     let mut reader = StreamReader::new(file, deframer);
//!
//!     reader.process_all(|payload| {
//!         println!("Read message: {} bytes", payload.len());
//!         Ok(())
//!     })?;
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

pub mod checksum;
pub mod error;
pub mod framing;
pub mod reader;
pub mod traits;
pub mod writer;

// Re-export the main public API for user convenience.
pub use checksum::NoChecksum;
pub use error::{Error, Result};
pub use framing::{DefaultDeframer, DefaultFramer, Deframer, Framer, SafeTakeDeframer, UnsafeDeframer};
pub use reader::{Messages, StreamReader};
pub use traits::StreamSerialize;
pub use writer::StreamWriter;

#[cfg(feature = "xxhash")]
pub use checksum::XxHash64;
#[cfg(any(feature = "xxhash", feature = "crc32"))]
pub use framing::{ChecksumDeframer, ChecksumFramer};

#[cfg(feature = "crc32")]
pub use checksum::Crc32;

#[cfg(feature = "crc16")]
pub use checksum::Crc16;
