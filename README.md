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
* **High-Performance Optimizations:** Write batching and zero-allocation reading for demanding use cases.
* **Comprehensive Benchmarking:** Extensive performance analysis with feature-gated benchmarks for all checksum algorithms.
* **Rust-Native:** Built entirely in Rust, leveraging its performance and safety features.

## Proposed Stream Format

Each message in the stream follows this structure:

`[ 4-byte Payload Length (u32, little-endian) | 8-byte Checksum (u64, little-endian, if enabled) | FlatBuffer Payload ]`

* **Payload Length:** Specifies the size of the FlatBuffer payload in bytes.
* **Checksum:** A hash of the FlatBuffer payload, used for integrity verification. Currently defaults to `xxh3_64` for its speed and reliability.
* **FlatBuffer Payload:** The raw bytes of the serialized FlatBuffer message.

## Quick Start

Add `flatstream-rs` to your `Cargo.toml`:

```toml
[dependencies]
flatstream-rs = "2.5.0" # Latest version with Processor API
flatbuffers = "24.3.25" # Or the version you are using
xxhash-rust = { version = "0.8", features = ["xxh3"] } # If using xxh3_64

### Basic Usage

```rust
use flatstream_rs::*;
use flatbuffers::FlatBufferBuilder;
use std::fs::File;
use std::io::{BufReader, BufWriter};

#[derive(StreamSerialize)]
struct TelemetryEvent {
    timestamp: u64,
    value: f64,
    device_id: String,
}

fn main() -> Result<()> {
    // Write messages
    let file = File::create("telemetry.bin")?;
    let writer = BufWriter::new(file);
    let framer = DefaultFramer;
    let mut stream_writer = StreamWriter::new(writer, framer);
    let mut builder = FlatBufferBuilder::new();

    for i in 0..100 {
        let event = TelemetryEvent {
            timestamp: i * 1000,
            value: i as f64 * 1.5,
            device_id: format!("sensor-{}", i % 10),
        };

        // External builder management for zero-allocation writes
        builder.reset();
        event.serialize(&mut builder)?;
        builder.finish(data, None);
        stream_writer.write(&mut builder)?;
    }

    // Read messages with zero-copy processing
    let file = File::open("telemetry.bin")?;
    let reader = BufReader::new(file);
    let deframer = DefaultDeframer;
    let mut stream_reader = StreamReader::new(reader, deframer);

    let mut count = 0;
    stream_reader.process_all(|payload| {
        // Zero-copy access to message data
        let event = flatbuffers::get_root::<telemetry::Event>(payload)?;
        println!("Event {}: timestamp={}, value={}", 
                count, event.timestamp(), event.value());
        count += 1;
        Ok(())
    })?;

    println!("Processed {} events", count);
    Ok(())
}
```

### Advanced Usage

For manual iteration control and early termination:

```rust
let mut messages = stream_reader.messages();
while let Some(payload) = messages.next()? {
    let event = flatbuffers::get_root::<telemetry::Event>(payload)?;
    
    if event.value() > 100.0 {
        println!("High value detected, stopping early");
        break;
    }
    
    process_event(event)?;
}
```

## Available Features

The library supports several optional features to customize functionality:

- **`xxhash`**: Enables XXHash64 checksum support (8 bytes, default: disabled)
- **`crc32`**: Enables CRC32 checksum support (4 bytes, default: disabled)
- **`crc16`**: Enables CRC16 checksum support (2 bytes, default: disabled)
- **`all_checksums`**: Enables all available checksum algorithms for testing and development
- **`async`**: Enables async I/O support with tokio (default: disabled)

Example with multiple features:
```toml
[dependencies]
flatstream-rs = { version = "0.1.0", features = ["xxhash", "crc32", "crc16"] }
```

For comprehensive testing with all checksums enabled:
```bash
cargo test --features all_checksums
cargo bench --features all_checksums  # Run comprehensive benchmarks
```

## Sized Checksums

The library supports checksums of different sizes to optimize for different use cases:

- **CRC16 (2 bytes)**: Perfect for high-frequency small messages (75% less overhead than XXHash64)
- **CRC32 (4 bytes)**: Good balance for medium-sized messages (50% less overhead than XXHash64)  
- **XXHash64 (8 bytes)**: Best for large, critical messages (maximum integrity)

All checksums are pluggable and composable, allowing you to choose the optimal size for your specific use case.
