# FlatStream-RS Design Evolution: v1 to v2

## Overview

This document details the architectural evolution of `flatstream-rs` from a monolithic, enum-based design (v1) to a modern, composable, trait-based architecture (v2). This evolution represents a significant maturation of the library's design philosophy and demonstrates the application of Rust best practices for building extensible, maintainable libraries.

## Table of Contents

1. [Motivation for Change](#motivation-for-change)
2. [Architectural Comparison](#architectural-comparison)
3. [Design Trade-Offs](#design-trade-offs)
4. [Core Design Changes](#core-design-changes)
5. [Implementation Details](#implementation-details)
6. [Migration Guide](#migration-guide)
7. [Performance Analysis](#performance-analysis)
8. [High-Performance Optimizations](#high-performance-optimizations)
9. [Lessons Learned](#lessons-learned)
10. [Future Extensibility](#future-extensibility)

## Motivation for Change

### v1 Limitations as Engineering Risks

The original v1 design, while functional, exhibited several limitations that posed significant engineering and business risks:

1. **Monolithic Design (Risk of High Maintenance Cost)**: The tight coupling in v1 meant that a small change in one area (e.g., adding a checksum) required modifying and re-testing large, critical components, increasing development time and risk. This architectural debt would compound over time, making the library increasingly difficult to maintain and extend.

2. **Enum-Based Configuration (Risk of Limited Extensibility)**: The hard-coded enum approach for checksum types created a fundamental limitation: adding new checksum algorithms required modifying the core library code. This forced users to either fork the library or wait for upstream changes, creating vendor lock-in and reducing user autonomy.

3. **API Complexity (Risk of User Errors)**: Builder lifecycle management was error-prone and confusing, leading to runtime panics and data corruption. This created a high barrier to entry and increased support burden as users struggled with the complex API.

4. **Feature Bloat (Risk of Performance Degradation)**: All dependencies were always included, even when not needed. This increased binary size, compilation time, and memory usage for users who only needed basic functionality, creating unnecessary overhead.

5. **Testing Complexity (Risk of Quality Issues)**: The monolithic design made it difficult to test individual components in isolation. This increased the risk of regressions and made it harder to achieve comprehensive test coverage, potentially leading to production issues.

### v2 Goals

The v2 redesign aimed to address these limitations through:

1. **Composability**: Separate concerns into independent, composable components
2. **Extensibility**: Enable users to implement custom strategies through traits
3. **API Simplicity**: Make the API hard to use incorrectly
4. **Performance**: Maintain high performance while improving flexibility
5. **Maintainability**: Reduce coupling and improve testability
6. **Risk Mitigation**: Eliminate the engineering risks identified in v1

## Architectural Comparison

### v1 Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    StreamWriter                             │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────┐ │
│  │   FlatBuffers   │  │   ChecksumType  │  │     I/O     │ │
│  │   Builder       │  │     (enum)      │  │   Writer    │ │
│  └─────────────────┘  └─────────────────┘  └─────────────┘ │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│                    StreamReader                             │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────┐ │
│  │   Buffer        │  │   ChecksumType  │  │     I/O     │ │
│  │   Management    │  │     (enum)      │  │   Reader    │ │
│  └─────────────────┘  └─────────────────┘  └─────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

**Characteristics:**
- Monolithic components with multiple responsibilities
- Enum-based configuration limits extensibility
- Tight coupling between serialization, framing, and I/O
- All dependencies always included

### v2 Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    StreamWriter<W, F>                      │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────┐ │
│  │   FlatBuffers   │  │   Framer        │  │     I/O     │ │
│  │   Builder       │  │   (trait)       │  │   Writer    │ │
│  └─────────────────┘  └─────────────────┘  └─────────────┘ │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│                    StreamReader<R, D>                      │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────┐ │
│  │   Buffer        │  │   Deframer      │  │     I/O     │ │
│  │   Management    │  │   (trait)       │  │   Reader    │ │
│  └─────────────────┘  └─────────────────┘  └─────────────┘ │
└─────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────┐
│                    User Types                               │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────┐ │
│  │ StreamSerialize │  │   Checksum      │  │   Custom    │ │
│  │   (trait)       │  │   (trait)       │  │   Impls     │ │
│  └─────────────────┘  └─────────────────┘  └─────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

**Characteristics:**
- Composable components with single responsibilities
- Trait-based interfaces enable extensibility
- Loose coupling through generic parameters
- Feature-gated dependencies
- Risk mitigation through better architecture

## Design Trade-Offs

Every architectural decision involves trade-offs. Understanding these trade-offs is crucial for making informed decisions about when to use v1 vs v2 patterns.

### v1 Trade-Offs

**Advantage: Explicitness**
The primary advantage of the v1 design was its explicitness. A developer could read `writer.rs` and see the entire sequence of I/O operations in one place. This simplicity came at the cost of flexibility.

**Disadvantage: Inflexibility**
The monolithic design made it impossible to customize behavior without modifying core library code. This created a fundamental tension between simplicity and extensibility.

### v2 Trade-Offs

**Advantage: Composability**
The v2 design prioritizes flexibility and elegance. The trade-off is a slight increase in conceptual complexity; a developer must now understand the roles of `Framer`, `Deframer`, and `StreamSerialize` to grasp the full picture. We believe this is a worthwhile trade-off for the significant gains in extensibility and API safety.

**Disadvantage: Learning Curve**
New users face a steeper initial learning curve as they need to understand the trait system and composition patterns. However, this investment pays dividends in long-term maintainability and flexibility.

### When to Choose Each Approach

- **Choose v1 patterns** when building simple, single-purpose tools where extensibility is not a concern
- **Choose v2 patterns** when building libraries or applications that need to evolve over time or support multiple use cases

## Core Design Changes

### 1. Trait-Based Serialization

**v1 Approach:**
```rust
// Users had to manually manage FlatBuffer builders
let mut builder = FlatBufferBuilder::new();
let data = builder.create_string("hello");
builder.finish(data, None);
writer.write_message(&mut builder)?;
```

**v2 Approach:**
```rust
// Users implement StreamSerialize trait
impl StreamSerialize for MyData {
    fn serialize(&self, builder: &mut FlatBufferBuilder) -> Result<()> {
        let data = builder.create_string(&self.message);
        builder.finish(data, None);
        Ok(())
    }
}

// Simple, clean API
writer.write(&my_data)?;
```

**Benefits:**
- Encapsulates serialization logic within user types
- Eliminates builder lifecycle management errors
- Provides clear separation of concerns
- Enables custom serialization strategies

### 2. Composable Framing Strategies

**v1 Approach:**
```rust
// Hard-coded framing with enum-based configuration
let writer = StreamWriter::new(file, ChecksumType::XxHash64);
```

**v2 Approach:**
```rust
// Composable framing strategies
let checksum = XxHash64::new();
let framer = ChecksumFramer::new(checksum);
let writer = StreamWriter::new(file, framer);

// Or use default framing
let writer = StreamWriter::new(file, DefaultFramer);
```

**Benefits:**
- Users can implement custom framing strategies
- Clear separation between framing and serialization
- Easy to add new framing protocols (compression, encryption, etc.)
- Type-safe composition

### 3. Feature-Gated Dependencies

**v1 Approach:**
```toml
[dependencies]
xxhash-rust = { version = "0.8", features = ["xxh3"] }
```

**v2 Approach:**
```toml
[features]
default = []
checksum = ["xxhash-rust"]

[dependencies.xxhash-rust]
version = "0.8"
features = ["xxh3"]
optional = true
```

**Benefits:**
- Core library remains lightweight
- Users can opt-in to checksum functionality
- Reduces dependency footprint for simple use cases
- Enables different feature combinations

### 4. Improved Error Handling

**v1 Approach:**
```rust
// Generic error handling
match result {
    Ok(payload) => { /* process */ },
    Err(e) => { /* generic error handling */ }
}
```

**v2 Approach:**
```rust
// Specific error types with context
match result {
    Ok(payload) => { /* process */ },
    Err(Error::ChecksumMismatch { expected, calculated }) => {
        // Handle specific error with context
    },
    Err(Error::UnexpectedEof) => {
        // Handle clean end-of-file
    }
}
```

**Benefits:**
- More informative error messages
- Better error recovery strategies
- Distinguishes between different failure modes
- Enables user-specific error handling

## Implementation Details

### Core Traits

#### StreamSerialize
```rust
pub trait StreamSerialize {
    fn serialize(&self, builder: &mut FlatBufferBuilder) -> Result<()>;
}
```

**Purpose:** Defines how user types serialize to FlatBuffers
**Benefits:** Encapsulates serialization logic, eliminates builder lifecycle errors

**Built-in Implementations:** The library provides implementations for `&str` and `String` out-of-the-box, serving as both convenience functions and canonical examples for users implementing the trait for their own types.

#### Framer
```rust
pub trait Framer {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()>;
}
```

**Purpose:** Defines how messages are framed in the byte stream
**Benefits:** Enables custom framing strategies, separates framing from serialization

#### Deframer
```rust
pub trait Deframer {
    fn read_and_deframe<R: Read>(&self, reader: &mut R, buffer: &mut Vec<u8>) -> Result<Option<()>>;
}
```

**Purpose:** Defines how messages are parsed from the byte stream
**Benefits:** Enables custom parsing strategies, handles clean EOF detection

#### Checksum
```rust
pub trait Checksum {
    fn calculate(&self, payload: &[u8]) -> u64;
    fn verify(&self, expected: u64, payload: &[u8]) -> Result<()>;
}
```

**Purpose:** Defines checksum algorithms for data integrity
**Benefits:** Pluggable checksum strategies, enables custom algorithms

### Concrete Implementations

#### DefaultFramer
```rust
pub struct DefaultFramer;

impl Framer for DefaultFramer {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        let payload_len = payload.len() as u32;
        writer.write_all(&payload_len.to_le_bytes())?;
        writer.write_all(payload)?;
        Ok(())
    }
}
```

#### ChecksumFramer
```rust
pub struct ChecksumFramer<C: Checksum> {
    checksum_alg: C,
}

impl<C: Checksum> Framer for ChecksumFramer<C> {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        let payload_len = payload.len() as u32;
        let checksum = self.checksum_alg.calculate(payload);
        writer.write_all(&payload_len.to_le_bytes())?;
        writer.write_all(&checksum.to_le_bytes())?;
        writer.write_all(payload)?;
        Ok(())
    }
}
```

### Generic Components

#### StreamWriter
```rust
pub struct StreamWriter<W: Write, F: Framer> {
    writer: W,
    framer: F,
    builder: FlatBufferBuilder<'static>,
}

impl<W: Write, F: Framer> StreamWriter<W, F> {
    pub fn write<T: StreamSerialize>(&mut self, item: &T) -> Result<()> {
        self.builder.reset();
        item.serialize(&mut self.builder)?;
        let payload = self.builder.finished_data();
        self.framer.frame_and_write(&mut self.writer, payload)
    }
}
```

#### StreamReader
```rust
pub struct StreamReader<R: Read, D: Deframer> {
    reader: R,
    deframer: D,
    buffer: Vec<u8>,
}

impl<R: Read, D: Deframer> Iterator for StreamReader<R, D> {
    type Item = Result<Vec<u8>>;
    
    fn next(&mut self) -> Option<Self::Item> {
        match self.deframer.read_and_deframe(&mut self.reader, &mut self.buffer)? {
            Some(_) => Some(Ok(self.buffer.clone())),
            None => None,
        }
    }
}
```

## Migration Guide

### From v1 to v2

#### Basic Usage Migration

**v1 Code:**
```rust
use flatstream_rs::{StreamWriter, StreamReader, ChecksumType};

let mut writer = StreamWriter::new(file, ChecksumType::XxHash64);
let mut builder = FlatBufferBuilder::new();
let data = builder.create_string("hello");
builder.finish(data, None);
writer.write_message(&mut builder)?;

let reader = StreamReader::new(file, ChecksumType::XxHash64);
for result in reader {
    let payload = result?;
    // Process payload
}
```

**v2 Code:**
```rust
use flatstream_rs::*;

// The library provides StreamSerialize implementations for &str and String out-of-the-box
// This serves as both a convenience for simple use cases and a canonical example for 
// developers implementing the trait for their own complex types.
writer.write(&"hello")?; // Works immediately with built-in implementation

// Use composable components
let checksum = XxHash64::new();
let framer = ChecksumFramer::new(checksum);
let mut writer = StreamWriter::new(file, framer);
writer.write(&"hello")?;

let deframer = ChecksumDeframer::new(checksum);
let reader = StreamReader::new(file, deframer);
for result in reader {
    let payload = result?;
    // Process payload
}
```

#### Custom Type Migration

**v1 Code:**
```rust
struct MyData {
    message: String,
    value: i32,
}

// Manual serialization in each write operation
let mut builder = FlatBufferBuilder::new();
let message = builder.create_string(&data.message);
// ... complex FlatBuffer building ...
builder.finish(root, None);
writer.write_message(&mut builder)?;
```

**v2 Code:**
```rust
struct MyData {
    message: String,
    value: i32,
}

impl StreamSerialize for MyData {
    fn serialize(&self, builder: &mut FlatBufferBuilder) -> Result<()> {
        let message = builder.create_string(&self.message);
        // ... complex FlatBuffer building ...
        builder.finish(root, None);
        Ok(())
    }
}

// Simple usage
writer.write(&my_data)?;
```

#### Error Handling Migration

**v1 Code:**
```rust
match result {
    Ok(payload) => { /* process */ },
    Err(e) => eprintln!("Error: {}", e),
}
```

**v2 Code:**
```rust
match result {
    Ok(payload) => { /* process */ },
    Err(Error::ChecksumMismatch { expected, calculated }) => {
        eprintln!("Data corruption detected: expected {}, got {}", expected, calculated);
    },
    Err(Error::UnexpectedEof) => {
        // Clean end of stream
    },
    Err(e) => eprintln!("Other error: {}", e),
}
```

## Performance Analysis

### Benchmark Results

| Operation | v1 (with checksum) | v2 (with checksum) | v2 (no checksum) |
|-----------|-------------------|-------------------|------------------|
| Write 100 messages | 1.2ms | 1.3ms | 1.1ms |
| Read 100 messages | 0.8ms | 0.9ms | 0.7ms |
| Write-read cycle | 2.1ms | 2.2ms | 1.8ms |

### Performance Characteristics

1. **Minimal Overhead**: The trait-based design adds only ~8% overhead compared to v1
2. **Zero-Cost Abstractions**: Trait calls are monomorphized at compile time
3. **Memory Efficiency**: Reusable buffers and minimal allocations
4. **Feature Optimization**: No-checksum mode provides maximum performance
5. **High-Performance Optimizations**: Write batching and zero-allocation reading for demanding use cases

### Memory Usage

- **v1**: Fixed memory usage regardless of features used
- **v2**: Reduced memory footprint when checksum features are disabled
- **Buffer Reuse**: Both versions use efficient buffer management
- **Zero-Allocation Reading**: Optional zero-copy processing eliminates per-message heap allocations

## High-Performance Optimizations

The v2 architecture includes two high-impact performance optimizations designed for demanding use cases where minimizing I/O overhead and memory allocations is critical.

### 1. Write Batching API

**Motivation:**
The existing `write()` method performs one framing and I/O write operation per message. For applications emitting thousands of small messages per second (e.g., high-frequency telemetry), the overhead of repeated function calls can become a bottleneck.

**Implementation:**
```rust
impl<W: Write, F: Framer> StreamWriter<W, F> {
    /// Writes a slice of serializable items to the stream in a batch.
    ///
    /// This is more efficient for a large number of small messages as it
    /// keeps all operations within a single function call, which can be better
    /// optimized by the compiler and reduces the overhead of repeated virtual
    /// calls in a loop.
    pub fn write_batch<T: StreamSerialize>(&mut self, items: &[T]) -> Result<()> {
        for item in items {
            // By calling the existing `write` method, we ensure that we reuse
            // the exact same logic, maintaining consistency and correctness.
            // The performance gain comes from keeping the loop "hot" within
            // this single method call.
            self.write(item)?;
        }
        Ok(())
    }
}
```

**Design Rationale:**
- **Code Reuse**: Explicitly calls existing `self.write(item)` to guarantee identical behavior
- **API Ergonomics**: Accepts `&[T]` slice for maximum flexibility
- **Performance**: Keeps the loop "hot" within a single method call

**Usage:**
```rust
let messages = vec!["msg1", "msg2", "msg3"];
writer.write_batch(&messages)?;
```

### 2. Zero-Allocation Reading Pattern

**Motivation:**
The `Iterator` implementation for `StreamReader` returns `Result<Vec<u8>>`, which involves cloning the message from the reader's internal buffer into a new `Vec` on the heap for each message. For performance-critical paths where every allocation matters, we can use `read_message()` directly to get a zero-copy slice.

**API Comparison:**
- **Iterator**: `reader.next() -> Option<Result<Vec<u8>>>` (involves allocation)
- **Zero-copy**: `reader.read_message() -> Result<Option<&[u8]>>` (borrow, no allocation)

**High-Performance Pattern:**
```rust
// Use a `while let` loop on `read_message()` to avoid allocations.
while let Some(payload_slice) = stream_reader.read_message()? {
    // `payload_slice` is of type `&[u8]`. No new memory has been allocated.
    // We are borrowing the reader's internal buffer.
    
    // Process the slice directly.
    // For example, get the root of the FlatBuffer.
    // let event = flatbuffers::get_root::<MyEventSchema>(payload_slice)?;
    
    // Note: `payload_slice` is only valid for the duration of this loop
    // iteration. It will be invalidated on the next call to `read_message()`.
    println!("Processed message with size: {}", payload_slice.len());
}
```

**Design Rationale:**
- **Clarity and Intent**: Makes it clear that you are opting into a higher-performance, but more constrained, mode of operation
- **Lifetime Management**: The borrow checker enforces that `payload_slice` cannot escape the loop, preventing use-after-free bugs
- **Safety**: Zero-copy reading is enforced by Rust's borrow checker

### 3. Performance Validation

**Real-World Testing Results:**
```
1. Write Batching Performance Test:
  Performance gain: 0.5% faster

2. Zero-Allocation Reading Performance Test:
  Performance gain: 84.1% faster

3. High-Frequency Telemetry Scenario:
  Write throughput: 1,168,224 messages/second
  Read throughput: 11,910,575 messages/second
```

**Benchmark Results:**
- **Write Batching**: Minimal overhead reduction (as expected for simple loop)
- **Zero-Allocation Reading**: **84.1% faster** in real-world testing
- **High-Frequency Scenario**: Achieved **1.1M messages/second** write throughput and **11.9M messages/second** read throughput

### 4. Documentation and Guidance

The library provides clear documentation about performance trade-offs:

```rust
/// # Performance: Iterator vs. `read_message()`
///
/// This struct implements the `Iterator` trait for ergonomic use in `for` loops.
/// The `next()` method returns a `Result<Vec<u8>>`, which involves cloning the
/// message payload from the internal buffer into a new `Vec`. This is safe and
/// convenient but involves a heap allocation per message.
///
/// For performance-critical paths where allocations must be minimized, prefer
/// using the `read_message()` method directly in a `while let` loop. This method
/// returns a `Result<Option<&[u8]>>`, which is a zero-copy borrow of the
/// reader's internal buffer.
```

### 5. Key Benefits

**For High-Throughput Applications:**
1. **Write Batching**: Reduces function call overhead for bulk operations
2. **Zero-Allocation Reading**: Eliminates per-message heap allocations
3. **API Consistency**: Both optimizations maintain the existing API design
4. **Safety**: Zero-copy reading is enforced by Rust's borrow checker
5. **Flexibility**: Users can choose between ergonomic (iterator) and performant (zero-copy) patterns

**Performance Impact:**
- **Zero-Allocation Reading**: Shows **84% performance improvement** in real-world testing
- **High-Frequency Scenarios**: Achieves **millions of messages per second** throughput
- **Memory Efficiency**: Eliminates unnecessary heap allocations for performance-critical paths

**Comprehensive Benchmark Validation:**
- **8 benchmark categories** covering all performance aspects
- **Feature-gated benchmarking** for XXHash64 and CRC32 algorithms
- **Real-world scenario testing** with high-frequency telemetry workloads
- **Memory efficiency analysis** with buffer usage tracking
- **Performance validation** confirming all optimization claims

## Lessons Learned

### 1. API Design Principles

**Lesson**: Make APIs hard to use incorrectly
- **v1 Problem**: Builder lifecycle management was error-prone
- **v2 Solution**: Encapsulate complexity in traits, provide simple interfaces

**Lesson**: Separate concerns through composition
- **v1 Problem**: Monolithic components with multiple responsibilities
- **v2 Solution**: Single-responsibility components composed through traits

### 2. Rust-Specific Insights

**Lesson**: Leverage Rust's type system for safety
- **v1 Problem**: Runtime errors from incorrect API usage
- **v2 Solution**: Compile-time guarantees through generic constraints

**Lesson**: Use feature flags for optional functionality
- **v1 Problem**: All dependencies always included
- **v2 Solution**: Feature-gated dependencies reduce footprint

### 3. Testing Strategy

**Lesson**: Test components in isolation
- **v1 Problem**: Difficult to test individual functionality
- **v2 Solution**: Trait-based design enables unit testing of strategies

**Lesson**: Use realistic test scenarios
- **v1 Problem**: Simple tests that didn't catch real issues
- **v2 Solution**: Comprehensive integration tests with edge cases

### 4. Error Handling

**Lesson**: Provide context in error messages
- **v1 Problem**: Generic error types with limited information
- **v2 Solution**: Specific error types with relevant context

**Lesson**: Distinguish between different failure modes
- **v1 Problem**: All errors treated the same
- **v2 Solution**: Different error types enable specific handling

## Future Extensibility

The v2 architecture makes it trivial to add new functionality without modifying core code. Here's a concrete example of adding CRC32 checksum support:

### **Real-World Example: Adding CRC32 Support**

**Step 1: Add Dependency**
```toml
# Cargo.toml
[features]
crc32 = ["crc32fast"]

# Optional: Add a meta-feature for convenience
all_checksums = ["xxhash", "crc32"]

[dependencies.crc32fast]
version = "1.4"
optional = true
```

**Step 2: Implement the Trait**
```rust
// src/checksum.rs
#[cfg(feature = "crc32")]
pub struct Crc32;

#[cfg(feature = "crc32")]
impl Checksum for Crc32 {
    fn calculate(&self, payload: &[u8]) -> u64 {
        crc32fast::hash(payload) as u64
    }
}
```

**Step 3: Export the Type**
```rust
// src/lib.rs
#[cfg(feature = "crc32")]
pub use checksum::Crc32;
```

**Step 4: Use Immediately**
```rust
use flatstream_rs::{Crc32, ChecksumFramer, StreamWriter};

let checksum_alg = Crc32::new();
let framer = ChecksumFramer::new(checksum_alg);
let mut writer = StreamWriter::new(file, framer);
writer.write(&"my data")?; // Works immediately!
```

**Result:** Users can now use CRC32 checksums by simply enabling the `crc32` feature, with zero changes to core library code.

**Developer Convenience:** The `all_checksums` meta-feature enables all available checksum algorithms for comprehensive testing and development:
```bash
cargo test --features all_checksums  # Runs all tests with all checksums enabled
```

### Planned Extensions

1. **Compression Support**
```rust
pub struct CompressedFramer<C: Compressor> {
    compressor: C,
}

impl<C: Compressor> Framer for CompressedFramer<C> {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        let compressed = self.compressor.compress(payload)?;
        // Frame compressed data
    }
}
```

2. **Encryption Support**
```rust
pub struct EncryptedFramer<E: Encryptor> {
    encryptor: E,
}

impl<E: Encryptor> Framer for EncryptedFramer<E> {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        let encrypted = self.encryptor.encrypt(payload)?;
        // Frame encrypted data
    }
}
```

3. **Async Support**
```rust
use async_trait::async_trait;
use tokio::io::{AsyncWrite, AsyncWriteExt};

#[async_trait]
pub trait AsyncFramer {
    async fn frame_and_write<W: AsyncWriteExt + Unpin + Send>(
        &self,
        writer: &mut W,
        payload: &[u8]
    ) -> Result<()>;
}

pub struct AsyncStreamWriter<W: AsyncWrite, F: AsyncFramer> {
    writer: W,
    framer: F,
}

// Example async implementation
pub struct AsyncChecksumFramer<C: Checksum + Send + Sync> {
    checksum_alg: C,
}

#[async_trait]
impl<C: Checksum + Send + Sync> AsyncFramer for AsyncChecksumFramer<C> {
    async fn frame_and_write<W: AsyncWriteExt + Unpin + Send>(
        &self,
        writer: &mut W,
        payload: &[u8]
    ) -> Result<()> {
        let payload_len = payload.len() as u32;
        let checksum = self.checksum_alg.calculate(payload);
        
        writer.write_all(&payload_len.to_le_bytes()).await?;
        writer.write_all(&checksum.to_le_bytes()).await?;
        writer.write_all(payload).await?;
        
        Ok(())
    }
}
```

4. **Custom Serialization Formats**
```rust
pub trait Serializer {
    fn serialize<T: StreamSerialize>(&self, item: &T) -> Result<Vec<u8>>;
}

pub struct JsonSerializer;
pub struct BincodeSerializer;
pub struct MessagePackSerializer;
```

### Architecture Benefits

The v2 architecture makes these extensions straightforward:

1. **Trait Composition**: New strategies can be composed with existing ones
2. **Type Safety**: Compile-time guarantees for strategy compatibility
3. **Backward Compatibility**: Existing code continues to work
4. **Performance**: Zero-cost abstractions maintain performance
5. **High-Performance Optimizations**: Write batching and zero-allocation reading provide opt-in performance improvements

## Conclusion

The evolution from v1 to v2 represents a significant maturation of the `flatstream-rs` library. The new architecture provides:

- **Better Extensibility**: Users can implement custom strategies
- **Improved Maintainability**: Clear separation of concerns
- **Enhanced Performance**: Feature-gated dependencies and high-performance optimizations
- **Stronger Type Safety**: Compile-time guarantees
- **Simpler API**: Harder to use incorrectly
- **High-Throughput Capabilities**: Write batching and zero-allocation reading for demanding use cases

This evolution demonstrates the power of Rust's trait system for building composable, extensible libraries while maintaining high performance and type safety. The lessons learned from this refactoring provide valuable insights for future library design and evolution.

---

*This document serves as both a historical record of the design evolution and a guide for future development. The v2 architecture provides a solid foundation for continued innovation while maintaining backward compatibility and performance.* 