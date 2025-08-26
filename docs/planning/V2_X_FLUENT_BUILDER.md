Design Document: A Fluent, Zero-Cost Builder API for Composable Stream Configuration

Version: 1.0  
Status: Proposed  
Author: Implementation Team

1.0 Overview and Motivation

This document proposes a fluent builder API for constructing StreamWriter and StreamReader instances in flatstream-rs. The goal is to enhance ergonomics, readability, and discoverability when composing multiple adapters (e.g., checksums, payload bounds, observers), while preserving the library’s core commitments to zero-copy, zero-cost abstractions, and static dispatch.

1.1 Current State: Powerful but Verbose Composition

The current API exposes composable adapter types (e.g., BoundedFramer, ObserverFramer, ChecksumFramer for writes; BoundedDeframer, ObserverDeframer, ChecksumDeframer for reads). Users combine these by manually nesting constructor calls to build a pipeline, and then hand that concrete type to StreamWriter or StreamReader. This is expressive and efficient, but can become verbose and hard to scan as the number of adapters grows.

Example (manual composition):

```rust
use flatstream::{DefaultFramer, StreamWriter, XxHash64};
use flatstream::framing::{BoundedFramer, ObserverFramer, ChecksumFramer};
use std::io::BufWriter;

let writer = BufWriter::new(std::fs::File::create("data.bin")?);

let framer = BoundedFramer::new(
    ObserverFramer::new(
        ChecksumFramer::new(XxHash64::new()),
        |payload| eprintln!("writing {} bytes", payload.len()),
    ),
    1024,
);

let mut stream_writer = StreamWriter::new(writer, framer);
```

1.2 Problem Statement: Readability and Discoverability Gaps

- Readability: Deeply nested constructor expressions read inside-out; intent is not immediately obvious.
- Discoverability: Users must already know adapter names and signatures to chain them correctly.
- Maintainability: Adding/removing/reordering adapters is error-prone and noisy.

1.3 Goal: A Fluent, Discoverable, Zero-Cost Builder

- Ergonomics: Replace nested constructors with a fluent, top-down configuration flow.
- Discoverability: IDE completion on builder methods reveals available adapters and parameters.
- Zero-Cost: Compile-time-only construction; same concrete types and static dispatch as manual composition.

2.0 Proposed Design: StreamWriter::builder and StreamReader::builder

We introduce builder types that accumulate adapter configuration fluently and produce the same concrete types as manual composition.

2.1 Writer Builder API

```rust
// In src/writer.rs
use std::io::Write;
use crate::framing::{Framer, DefaultFramer, BoundedFramer, ObserverFramer};
use crate::checksum::Checksum;

pub struct StreamWriterBuilder<W: Write, F: Framer> {
    writer: W,
    framer: F,
}

impl<W: Write> StreamWriterBuilder<W, DefaultFramer> {
    /// Entry point for fluent writer configuration.
    pub fn new(writer: W) -> Self {
        Self { writer, framer: DefaultFramer }
    }
}

impl<W: Write, F: Framer> StreamWriterBuilder<W, F> {
    /// Add a checksum layer for data integrity.
    /// Available when a checksum feature is enabled (e.g., xxhash, crc32, crc16).
    pub fn with_checksum<C: Checksum>(self, checksum: C)
        -> StreamWriterBuilder<W, crate::framing::ChecksumFramer<C>>
    {
        // Note: ChecksumFramer currently does not wrap an inner Framer; see 4.2.
        StreamWriterBuilder { writer: self.writer, framer: crate::framing::ChecksumFramer::new(checksum) }
    }

    /// Enforce a maximum payload size.
    pub fn with_max_payload_size(self, limit: usize) -> StreamWriterBuilder<W, BoundedFramer<F>> {
        StreamWriterBuilder { writer: self.writer, framer: BoundedFramer::new(self.framer, limit) }
    }

    /// Observe payloads on the write path without copying.
    pub fn with_observer<Cb: Fn(&[u8])>(self, callback: Cb)
        -> StreamWriterBuilder<W, ObserverFramer<F, Cb>>
    {
        StreamWriterBuilder { writer: self.writer, framer: ObserverFramer::new(self.framer, callback) }
    }

    /// Finalize and construct a StreamWriter with the accumulated configuration.
    pub fn build(self) -> crate::writer::StreamWriter<W> {
        crate::writer::StreamWriter::new(self.writer, self.framer)
    }
}
```

2.2 Reader Builder API

```rust
// In src/reader.rs
use std::io::Read;
use crate::framing::{Deframer, DefaultDeframer, BoundedDeframer, ObserverDeframer};
use crate::checksum::Checksum;

pub struct StreamReaderBuilder<R: Read, D: Deframer> {
    reader: R,
    deframer: D,
}

impl<R: Read> StreamReaderBuilder<R, DefaultDeframer> {
    pub fn new(reader: R) -> Self { Self { reader, deframer: DefaultDeframer } }
}

impl<R: Read, D: Deframer> StreamReaderBuilder<R, D> {
    pub fn with_checksum<C: Checksum>(self, checksum: C)
        -> StreamReaderBuilder<R, crate::framing::ChecksumDeframer<C>>
    {
        StreamReaderBuilder { reader: self.reader, deframer: crate::framing::ChecksumDeframer::new(checksum) }
    }

    pub fn with_max_payload_size(self, limit: usize) -> StreamReaderBuilder<R, BoundedDeframer<D>> {
        StreamReaderBuilder { reader: self.reader, deframer: BoundedDeframer::new(self.deframer, limit) }
    }

    pub fn with_observer<Cb: Fn(&[u8])>(self, callback: Cb)
        -> StreamReaderBuilder<R, ObserverDeframer<D, Cb>>
    {
        StreamReaderBuilder { reader: self.reader, deframer: ObserverDeframer::new(self.deframer, callback) }
    }

    pub fn build(self) -> crate::reader::StreamReader<R, D> {
        crate::reader::StreamReader::new(self.reader, self.deframer)
    }
}

impl<R: Read> crate::reader::StreamReader<R, DefaultDeframer> {
    /// Fluent entry-point: StreamReader::builder(reader)
    pub fn builder(reader: R) -> StreamReaderBuilder<R, DefaultDeframer> {
        StreamReaderBuilder::new(reader)
    }
}
```

2.3 Fluent Entry Points

```rust
impl<W: std::io::Write> crate::writer::StreamWriter<W> {
    pub fn builder(writer: W) -> StreamWriterBuilder<W, DefaultFramer> {
        StreamWriterBuilder::new(writer)
    }
}

impl<R: std::io::Read> crate::reader::StreamReader<R, DefaultDeframer> {
    pub fn builder(reader: R) -> StreamReaderBuilder<R, DefaultDeframer> {
        StreamReaderBuilder::new(reader)
    }
}
```

3.0 Ergonomics: Before-and-After

Before (manual nesting):

```rust
let deframer = BoundedDeframer::new(
    ObserverDeframer::new(DefaultDeframer, |_| {}),
    1 << 20,
);
let mut reader = StreamReader::new(reader, deframer);
```

After (fluent builder):

```rust
let mut reader = StreamReader::builder(reader)
    .with_observer(|_| {})
    .with_max_payload_size(1 << 20)
    .build();
```

Benefits:
- Top-down readability; no parentheses pyramids.
- IDE-driven discoverability of options.
- Lower error surface when reordering/adding adapters.

4.0 Performance and Architectural Analysis

4.1 Zero-Cost Abstraction Justification

- The builder composes generic types at compile time; the built StreamWriter/StreamReader have identical concrete types as the manual approach.
- No dynamic dispatch; monomorphization and inlining preserve current performance.
- No extra allocations; builder is a small, stack-allocated helper that vanishes after build().

4.2 Adapter Composition Considerations

- Today, BoundedFramer and ObserverFramer already wrap an inner Framer; likewise for their Deframer counterparts.
- ChecksumFramer/ChecksumDeframer currently stand alone (no inner) and are typically used as the sole Framer/Deframer. Two paths exist:
  1) Minimal-change builder: allow with_checksum() to return a builder specialized over ChecksumFramer/ChecksumDeframer as the terminal framer/deframer. Other wrappers (bounded/observer) compose around it using existing inner APIs.
  2) Optional refactor (future): add inner to ChecksumFramer/ChecksumDeframer to allow any order in the chain, enabling patterns like observer→checksum→bounded in a single pass. This is purely ergonomic and does not affect performance guarantees.

5.0 Detailed Implementation Plan

5.1 File Manifest

- src/writer.rs: Add StreamWriterBuilder and StreamWriter::builder().
- src/reader.rs: Add StreamReaderBuilder and StreamReader::builder().
- src/lib.rs: Re-export builders (optional, for discoverability).
- examples/builder_example.rs: Demonstrate fluent configuration for both write/read paths with and without checksums.
- tests/builder_tests.rs: Verify functional equivalence with manual composition.
- benches/builder_benchmarks.rs: Compare manual vs builder (expected equivalence).

5.2 Backward Compatibility

- Additive API; no breaking changes.
- Manual composition remains fully supported.

5.3 Error Handling

- Builders surface the same Result types as StreamWriter/StreamReader at build-time only if an early error check becomes desirable (none planned initially, as builders primarily assemble types).

6.0 Verification Strategy

6.1 Unit Tests

- Builder chaining compiles across combinations: baseline, bounded, observer, checksum (feature-gated).
- Type equality assertions (where possible) to ensure built pipelines match manual equivalents.

6.2 Integration Tests

- Write a stream with builder-configured StreamWriter and read it back with builder-configured StreamReader. Validate byte-for-byte payloads and checksum verification (when enabled).
- Equivalence across all Deframers: DefaultDeframer, SafeTakeDeframer, UnsafeDeframer, and ChecksumDeframer (feature-gated), optionally wrapped with Bounded/Observer.

6.3 Performance Benchmarks

- Criterion benchmark comparing manual vs builder for read-only, write-only, and write-read cycles across small/medium/large payloads. Expect statistically indistinguishable results.

7.0 Future Work

- Optional adapter refactor to allow checksum adapters to wrap an inner Framer/Deframer for fully flexible ordering.
- Additional fluent steps (e.g., compression, encryption) if introduced as adapters in the future.
- Async builders (tokio feature) if/when async read/write variants are added.

8.0 Definition of Done

- Fluent builders implemented and exported; additive change only.
- New example added; existing examples optionally updated where readability improves.
- Tests cover builder paths across deframers and adapters; feature-gated combinations included.
- Benchmarks show equivalence with manual composition.
- Lints and formatting pass; CI green.

Appendix A: Minimal Usage Examples

```rust
// Writer: add checksum and bound
let mut writer = StreamWriter::builder(std::io::Cursor::new(Vec::new()))
    .with_checksum(flatstream::XxHash64::new())
    .with_max_payload_size(1 << 20)
    .build();

// Reader: observe and bound
let mut reader = StreamReader::builder(std::io::Cursor::new(Vec::new()))
    .with_observer(|p| eprintln!("{} bytes", p.len()))
    .with_max_payload_size(1 << 20)
    .build();
```


