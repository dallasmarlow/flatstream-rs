# `flatstream-rs`: High-Performance FlatBuffers Streaming with Data Integrity

[![Rust](https://github.com/dallasmarlow/flatstream-rs/actions/workflows/rust.yml/badge.svg)](https://github.com/dallasmarlow/flatstream-rs/actions/workflows/rust.yml)
[![Crates.io](https://img.shields.io/crates/v/flatstream-rs.svg)](https://crates.io/crates/flatstream-rs)
[![Docs.rs](https://docs.rs/flatstream-rs/badge.svg)](https://docs.rs/flatstream-rs)

## Project Description

`flatstream-rs` is a lightweight, high-performance Rust library designed for efficiently writing and reading streams of FlatBuffers messages to and from files or network streams. It focuses on simplicity without sacrificing core design principles, providing built-in support for optional data integrity checks via pluggable checksum algorithms.

This library is engineered for demanding use cases like telemetry data capture, where sub-millisecond updates need to be reliably stored and reprocessed.

## Why `flatstream-rs`?

While FlatBuffers excels at zero-copy deserialization, the official Rust `flatbuffers` crate primarily provides low-level building blocks. When working with continuous streams of messages (rather than single, self-contained buffers), developers often face the challenge of:

1.  **Framing:** How to delineate individual FlatBuffers messages within a continuous byte stream.
2.  **Data Integrity:** How to detect accidental corruption during storage or transmission.

`flatstream-rs` solves these problems by implementing a robust, size-prefixed framing format with an optional, pluggable checksum. This allows engineers to focus on their application logic, knowing that the underlying data stream is handled efficiently and reliably.

## Key Features

* **Efficient Streaming:** Designed for long-running data streams, enabling continuous writes and reads without loading the entire stream into memory.
* **Size-Prefixed Framing:** Each FlatBuffers message is automatically prefixed with its length, allowing for easy parsing of individual messages from the stream.
* **Optional Data Integrity Checks:** Integrate a checksum for each message to detect corruption.
    * **Pluggable Checksums:** Choose from different algorithms (e.g., `xxh3_64`) or disable checksumming entirely for maximum speed where integrity is handled by other layers.
* **Zero-Copy Read Support:** When reading, the library provides direct access to the FlatBuffers payload, leveraging FlatBuffers' zero-copy capabilities.
* **Simple & Direct API:** Designed for ease of use, minimizing boilerplate code for common streaming patterns.
* **Rust-Native:** Built entirely in Rust, leveraging its performance and safety features.

## Proposed Stream Format

Each message in the stream follows this structure:

`[ 4-byte Payload Length (u32, little-endian) | 8-byte Checksum (u64, little-endian, if enabled) | FlatBuffer Payload ]`

* **Payload Length:** Specifies the size of the FlatBuffer payload in bytes.
* **Checksum:** A hash of the FlatBuffer payload, used for integrity verification. Currently defaults to `xxh3_64` for its speed and reliability.
* **FlatBuffer Payload:** The raw bytes of the serialized FlatBuffer message.

## Usage Examples (Proposed API)

Add `flatstream-rs` to your `Cargo.toml`:

```toml
[dependencies]
flatstream-rs = "0.1.0" # Or the latest version
flatbuffers = "24.3.25" # Or the version you are using
xxhash-rust = { version = "0.8", features = ["xxh3"] } # If using xxh3_64
