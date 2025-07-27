//! # flatstream-rs
//!
//! A lightweight, high-performance Rust library designed for efficiently writing and reading streams of FlatBuffers messages to and from files or network streams. It focuses on simplicity without sacrificing core design principles, providing built-in support for optional data integrity checks via pluggable checksum algorithms.
//!
//! This library is engineered for demanding use cases like telemetry data capture, where sub-millisecond updates need to be reliably stored and reprocessed.
//!
//! ## Why `flatstream-rs`?
//!
//! While FlatBuffers excels at zero-copy deserialization, the official Rust `flatbuffers` crate primarily provides low-level building blocks. When working with continuous streams of messages (rather than single, self-contained buffers), developers often face the challenge of:
//!
//! 1. **Framing:** How to delineate individual FlatBuffers messages within a continuous byte stream.
//! 2. **Data Integrity:** How to detect accidental corruption during storage or transmission.
//!
//! `flatstream-rs` solves these problems by implementing a robust, size-prefixed framing format with an optional, pluggable checksum. This allows engineers to focus on their application logic, knowing that the underlying data stream is handled efficiently and reliably.
//!
//! ## Key Features
//!
//! * **Efficient Streaming:** Designed for long-running data streams, enabling continuous writes and reads without loading the entire stream into memory.
//! * **Size-Prefixed Framing:** Each FlatBuffers message is automatically prefixed with its length, allowing for easy parsing of individual messages from the stream.
//! * **Optional Data Integrity Checks:** Integrate a checksum for each message to detect corruption.
//!     * **Pluggable Checksums:** Choose from different algorithms (e.g., `xxh3_64`) or disable checksumming entirely for maximum speed where integrity is handled by other layers.
//! * **Zero-Copy Read Support:** When reading, the library provides direct access to the FlatBuffers payload, leveraging FlatBuffers' zero-copy capabilities.
//! * **Simple & Direct API:** Designed for ease of use, minimizing boilerplate code for common streaming patterns.
//! * **Rust-Native:** Built entirely in Rust, leveraging its performance and safety features.
//!
//! ## Stream Format
//!
//! Each message in the stream follows this structure:
//!
//! `[ 4-byte Payload Length (u32, little-endian) | 8-byte Checksum (u64, little-endian, if enabled) | FlatBuffer Payload ]`
//!
//! * **Payload Length:** Specifies the size of the FlatBuffer payload in bytes.
//! * **Checksum:** A hash of the FlatBuffer payload, used for integrity verification. Currently defaults to `xxh3_64` for its speed and reliability.
//! * **FlatBuffer Payload:** The raw bytes of the serialized FlatBuffer message.
//!
//! ## Quick Start
//!
//! Add `flatstream-rs` to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! flatstream-rs = "0.1.0"
//! flatbuffers = "24.3.25"
//! ```
//!
//! ### Writing a Stream
//!
//! ```rust
//! use std::fs::File;
//! use std::io::BufWriter;
//! use flatbuffers::FlatBufferBuilder;
//! use flatstream_rs::{StreamWriter, ChecksumType};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let file = File::create("data.bin")?;
//!     let writer = BufWriter::new(file);
//!     let mut stream_writer = StreamWriter::new(writer, ChecksumType::XxHash64);
//!
//!     let mut builder = FlatBufferBuilder::new();
//!     // ... build your FlatBuffer message ...
//!     // let root = builder.finish(your_root, None);
//!     // For this example, we'll create a simple string
//!     let data = builder.create_string("example data");
//!     builder.finish(data, None);
//!
//!     stream_writer.write_message(&mut builder)?;
//!     stream_writer.flush()?;
//!     Ok(())
//! }
//! ```
//!
//! ### Reading a Stream
//!
//! ```rust
//! use std::fs::File;
//! use std::io::BufReader;
//! use flatstream_rs::{StreamReader, ChecksumType};
//!
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let file = File::open("data.bin")?;
//!     let reader = BufReader::new(file);
//!     let stream_reader = StreamReader::new(reader, ChecksumType::XxHash64);
//!
//!     for result in stream_reader {
//!         match result {
//!             Ok(payload) => {
//!                 // Process the FlatBuffer payload
//!                 // Use flatbuffers::get_root to deserialize
//!             }
//!             Err(e) => {
//!                 eprintln!("Error reading stream: {}", e);
//!                 break;
//!             }
//!         }
//!     }
//!     Ok(())
//! }
//! ```
//!
//! ## Use Cases
//!
//! This library is ideally suited for:
//!
//! * **Telemetry Capturing Agents:** Long-running processes that need to emit continuous streams of data
//! * **Data Pipeline Ingestion:** Reliable storage of streaming data for later processing
//! * **High-Frequency Data Capture:** Sub-millisecond updates that need to be stored efficiently
//! * **Distributed Systems:** Reliable message streaming between services

pub mod checksum;
pub mod error;
pub mod reader;
pub mod writer;

// Re-export the main public API
pub use checksum::ChecksumType;
pub use error::{Error, Result};
pub use reader::StreamReader;
pub use writer::StreamWriter;
