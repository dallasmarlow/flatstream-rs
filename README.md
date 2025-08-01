# flatstream-rs (v2.5)

[![Rust](https://github.com/dallasmarlow/flatstream-rs/actions/workflows/rust.yml/badge.svg)](https://github.com/dallasmarlow/flatstream-rs/actions/workflows/rust.yml)
[![Crates.io](https://img.shields.io/crates/v/flatstream-rs.svg)](https://crates.io/crates/flatstream-rs)
[![Docs.rs](https://docs.rs/flatstream-rs/badge.svg)](https://docs.rs/flatstream-rs)

A lightweight, zero-copy, high-performance Rust library for streaming framed FlatBuffers.

`flatstream-rs` provides a trait-based architecture for efficiently writing and reading streams of FlatBuffer messages. It is designed for high-throughput, low-latency applications such as telemetry capture, network protocols, and high-speed data logging.

## Why flatstream-rs?

High-performance systems require efficient serialization and transmission of structured data. While FlatBuffers offers an excellent serialization format due to its zero-copy access and cross-platform compatibility, it does not inherently provide a streaming or framing protocol.

When writing multiple messages to a continuous byte stream (like a file or TCP socket), developers face several challenges:

- **Framing**: A mechanism is needed to delineate where one message ends and the next begins.
- **Memory Allocation Overhead**: Frequently allocating new buffers for every message creates excessive pressure on the memory allocator, introducing latency jitter and reducing throughput.
- **Data Integrity**: Streams may require checksums to validate that messages were not corrupted in transit or at rest.

`flatstream-rs` solves these problems by providing a standardized, highly optimized, and composable framing layer specifically designed for FlatBuffers in Rust. This library is engineered for demanding use cases like telemetry data capture, where sub-millisecond updates need to be reliably stored and reprocessed.

## Architecture and Design Principles

The library is designed around composability and zero-cost abstractions to maximize performance in demanding environments.

### Performance: Zero-Copy and Buffer Reuse

Performance is the primary design goal.

- **Writer Efficiency (Minimal Allocation)**: `StreamWriter` maintains ownership of a `FlatBufferBuilder`. This builder's memory allocation is reused for every write operation (the builder is `reset()`, not dropped and reallocated). This drastically minimizes heap allocations on the critical write path.
- **Reader Efficiency (Zero-Copy)**: `StreamReader` maintains a reusable read buffer and provides zero-copy access to the data. The `process_all` API delivers a borrowed slice (`&[u8]`) directly to the user, avoiding intermediate copies entirely.
- **Comprehensive Benchmarking**: Extensive performance analysis with feature-gated benchmarks for all checksum algorithms.

### Composability and Static Dispatch

The library utilizes a trait-based Strategy Pattern to separate concerns:

- **`StreamSerialize`**: Defines how user data is serialized into the FlatBufferBuilder.
- **`Framer` / `Deframer`**: Defines the wire/file format (e.g., `DefaultFramer` or `ChecksumFramer`).
- **`Checksum`**: Defines the algorithm used for data integrity (e.g., `XxHash64`, `Crc32`).

The core types (`StreamWriter`/`StreamReader`) are generic over these traits. This allows the Rust compiler to use monomorphization, resulting in static dispatch and eliminating the overhead of dynamic dispatch (vtable lookups) on the critical path.

## Installation

Add `flatstream-rs` and the `flatbuffers` dependency to your `Cargo.toml`:

```toml
[dependencies]
flatbuffers = "24.3.25" # Use the appropriate version
flatstream-rs = "0.1.0"
```

### Feature Flags

Data integrity checks (checksums) are optional and managed via feature flags.

- **`xxhash`**: Enables XXH3 (64-bit) checksum support. Highly recommended for high-performance integrity checks.
- **`crc32`**: Enables CRC32 checksum support.
- **`crc16`**: Enables CRC16 checksum support.
- **`all_checksums`**: Enables all available checksum algorithms for testing and development.

```toml
[dependencies]
# Example: Installing with XxHash support
flatstream-rs = { version = "0.1.0", features = ["xxhash"] }
```

For comprehensive testing with all checksums enabled:
```bash
cargo test --features all_checksums
cargo bench --features all_checksums  # Run comprehensive benchmarks
```

## Quick Start Example

### 1. Implementing StreamSerialize

Users must define how their data maps to a FlatBuffer builder by implementing the `StreamSerialize` trait.

```rust
use flatstream_rs::{StreamSerialize, Result};
use flatbuffers::FlatBufferBuilder;

// Your application data structure
struct TelemetryData {
    timestamp: u64,
    label: String,
}

impl StreamSerialize for TelemetryData {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>
    ) -> Result<()> {
        // This is where you use your FlatBuffers generated code.
        // Example:
        // let label = builder.create_string(&self.label);
        // let mut msg_builder = MyMessageBuilder::new(builder);
        // msg_builder.add_timestamp(self.timestamp);
        // msg_builder.add_label(label);
        // let offset = msg_builder.finish();

        // Simplified for demonstration: we just serialize the label.
        let offset = builder.create_string(&self.label);

        // Crucial: You must call finish() within serialize.
        builder.finish(offset, None);
        Ok(())
    }
}
```

### 2. Writing Data

The `StreamWriter` handles the framing and I/O.

```rust
use flatstream_rs::{StreamWriter, DefaultFramer, Result};
use std::io::Cursor;

fn write_data() -> Result<Vec<u8>> {
    let mut buffer = Vec::new();
    // In a real application, this should be a BufWriter<File> or similar
    let writer_backend = Cursor::new(&mut buffer);

    // Use the default framing strategy: [4-byte length | payload]
    let mut writer = StreamWriter::new(writer_backend, DefaultFramer);

    let data = TelemetryData {
        timestamp: 1659373987,
        label: "temp_sensor_1".to_string(),
    };

    // The writer manages the builder internally and reuses the allocation.
    writer.write(&data)?;
    writer.flush()?;

    Ok(buffer)
}
```

### 3. Reading Data (Zero-Copy)

The `StreamReader` provides a high-performance `process_all` API for zero-copy access.

```rust
use flatstream_rs::{StreamReader, DefaultDeframer, Result};
use std::io::Cursor;

fn read_data(data: Vec<u8>) -> Result<()> {
    let reader_backend = Cursor::new(data);
    let mut reader = StreamReader::new(reader_backend, DefaultDeframer);

    // High-performance, zero-copy processing
    reader.process_all(|payload: &[u8]| {
        // 'payload' is a slice pointing directly to the FlatBuffer message in the internal buffer.
        // You can now access the data using FlatBuffers verification/accessors.
        // Example: let msg = flatbuffers::root::<MyMessage>(payload).unwrap();

        println!("Read message of {} bytes.", payload.len());
        Ok(())
    })?;

    Ok(())
}
```

### Advanced: Manual Iteration Control

For cases requiring early termination or custom control flow:

```rust
let mut messages = reader.messages();
while let Some(payload) = messages.next()? {
    // Process message with zero-copy access
    if should_stop_early(payload) {
        break;
    }
}
```

## Advanced Usage: Data Integrity (Checksums)

To protect against data corruption, use the `ChecksumFramer` and `ChecksumDeframer`. This requires enabling a checksum feature (e.g., `xxhash`).

```rust
#[cfg(feature = "xxhash")]
{
    use flatstream_rs::{
        StreamWriter, ChecksumFramer, XxHash64, Result
    };
    use std::io::Cursor;

    fn write_protected() -> Result<()> {
        // 1. Define the checksum strategy (requires 'xxhash' feature)
        let checksum_alg = XxHash64::new();

        // 2. Create the framer
        let framer = ChecksumFramer::new(checksum_alg);

        // 3. Initialize the Writer
        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

        writer.write(&"A protected message")?;
        Ok(())
    }
}
```

When reading, use the corresponding `ChecksumDeframer`. It will automatically validate the integrity and return `Error::ChecksumMismatch` if the data is corrupted.

### Sized Checksums

The library supports checksums of different sizes to optimize for different use cases:

- **CRC16 (2 bytes)**: Perfect for high-frequency small messages (75% less overhead than XXHash64)
- **CRC32 (4 bytes)**: Good balance for medium-sized messages (50% less overhead than XXHash64)  
- **XXHash64 (8 bytes)**: Best for large, critical messages (maximum integrity)

All checksums are pluggable and composable, allowing you to choose the optimal size for your specific use case.

## Wire Format Specification

The format written to the stream is determined by the `Framer` implementation. `flatstream-rs` ensures all metadata (lengths and checksums) is written in Little Endian (LE) format to guarantee cross-platform consistency and interoperability.

### DefaultFramer Format

A simple, low-overhead format (4 bytes overhead).

```
[4 bytes LE: Payload Length (u32)] [Payload...]
```

### ChecksumFramer<T> Format

A robust format including data integrity validation. The overhead depends on the checksum algorithm (e.g., 4 bytes length + 8 bytes checksum for XxHash64).

```
[4 bytes LE: Payload Length (u32)] [N bytes LE: Checksum] [Payload...]
```

Where N is:
- 8 bytes for XXHash64 (u64)
- 4 bytes for CRC32 (u32)
- 2 bytes for CRC16 (u16)

## Performance Considerations

While `flatstream-rs` is optimized for high performance, achieving the lowest latency requires correct integration into your application architecture.

### Critical: I/O Buffering

`StreamWriter` and `StreamReader` operate directly on the underlying `W: Write` and `R: Read` types. They do not perform their own I/O buffering.

If you provide an unbuffered handle (like a raw `std::fs::File` or `std::net::TcpStream`), every write operation may result in a system call, significantly increasing latency and reducing throughput.

**Recommendation**: Always wrap file or network handles in `std::io::BufWriter` and `std::io::BufReader`.

```rust
use std::fs::File;
use std::io::BufWriter;
use flatstream_rs::{StreamWriter, DefaultFramer};

let file = File::create("telemetry.bin").unwrap();

// WRONG: Unbuffered I/O, potentially slow due to excessive syscalls
// let writer = StreamWriter::new(file, DefaultFramer);

// CORRECT: Buffered I/O
let buffered_writer = BufWriter::new(file);
let writer = StreamWriter::new(buffered_writer, DefaultFramer);
```

### Synchronous I/O

This library currently uses synchronous I/O based on standard Rust `Read`/`Write` traits. In highly concurrent, low-latency capture agents, blocking the main capture thread for I/O is undesirable.

**Recommendation**: In high-throughput agents, consider offloading the `StreamWriter` to a dedicated I/O thread, communicating with it via a fast MPSC channel (e.g., crossbeam or flume).