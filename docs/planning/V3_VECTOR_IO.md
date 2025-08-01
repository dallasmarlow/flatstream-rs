# V3.0 "Vectored I/O" Design Document

## Executive Summary

This document proposes the introduction of vectored I/O support to `flatstream-rs` v3.0, reducing syscall overhead by 50% while maintaining the library's zero-copy architecture. By leveraging `write_vectored` and `read_vectored`, we can combine multiple buffer operations into single system calls without introducing any memory copies.

## Table of Contents

1. [Motivation](#motivation)
2. [Design Goals](#design-goals)
3. [Technical Analysis](#technical-analysis)
4. [Implementation Plan](#implementation-plan)
5. [FlatBuffers Integration](#flatbuffers-integration)
6. [Performance Projections](#performance-projections)
7. [Migration Strategy](#migration-strategy)
8. [Future Considerations](#future-considerations)

## Motivation

### Current Syscall Overhead

The current implementation performs two separate system calls for each message:

```rust
// Current DefaultFramer implementation
fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
    let payload_len = payload.len() as u32;
    writer.write_all(&payload_len.to_le_bytes())?;  // Syscall #1
    writer.write_all(payload)?;                      // Syscall #2
    Ok(())
}
```

For high-frequency streaming applications (100k+ messages/sec), syscall overhead becomes significant:
- Each syscall: ~5-10μs overhead
- Per message: 2 syscalls = 10-20μs
- At 100k msg/sec: 1-2 seconds of CPU time spent in syscall overhead alone

### Why Vectored I/O?

Vectored I/O allows multiple non-contiguous memory regions to be written or read in a single system call:
- **Zero-copy**: No memory consolidation needed
- **Atomic operations**: All-or-nothing semantics
- **Kernel optimization**: Modern kernels optimize scatter-gather operations
- **FlatBuffers synergy**: Natural fit with FlatBuffers' segmented memory layout

## Design Goals

### Primary Objectives

1. **Reduce Syscall Overhead**: Achieve 50% reduction through vectored operations
2. **Maintain Zero-Copy**: No additional memory copies or allocations
3. **Preserve API Compatibility**: Opt-in enhancement, not a breaking change
4. **FlatBuffers Alignment**: Leverage FlatBuffers' buffer structure efficiently
5. **Composable Architecture**: Integrate seamlessly with existing framing strategies

### Non-Goals

- Async vectored I/O (future work)
- Custom kernel bypass (too complex)
- Breaking existing APIs

## Technical Analysis

### FlatBuffers Memory Layout

FlatBuffers produces segmented memory layouts that naturally align with vectored I/O:

```
FlatBuffer in Memory:
┌─────────────┬──────────────┬─────────────┬──────────────┐
│   VTable    │    Object    │   String    │   Vector     │
│  (inline)   │   (inline)   │  (offset)   │  (offset)    │
└─────────────┴──────────────┴─────────────┴──────────────┘

Vectored Write Opportunity:
IoSlice[0]: Length prefix (4 bytes)
IoSlice[1]: Checksum (0-8 bytes, optional)
IoSlice[2]: FlatBuffer payload (contiguous)
```

### Syscall Efficiency Analysis

```rust
// Syscall cost comparison (Linux x86_64)
write(fd, buf1, 4);        // ~5μs (length)
write(fd, buf2, 1000);     // ~5μs (payload)
// Total: ~10μs

writev(fd, [{buf1, 4}, {buf2, 1000}], 2);  // ~6μs
// Savings: 40% reduction
```

## Implementation Plan

### Phase 1: Core Vectored Framer

```rust
use std::io::{IoSlice, Write};

/// Zero-copy vectored framing strategy
pub struct VectoredFramer;

impl Framer for VectoredFramer {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        let len_bytes = (payload.len() as u32).to_le_bytes();
        
        // Create I/O vectors without copying
        let bufs = &[
            IoSlice::new(&len_bytes),
            IoSlice::new(payload),
        ];
        
        // Single vectored write
        match writer.write_vectored(bufs) {
            Ok(n) if n == 4 + payload.len() => Ok(()),
            Ok(_) => Err(Error::PartialWrite),
            Err(e) => Err(e.into()),
        }
    }
}

/// Vectored framing with checksums
pub struct VectoredChecksumFramer<C: Checksum> {
    checksum_alg: C,
    // Stack buffer for checksum (max 8 bytes)
    checksum_buf: [u8; 8],
}

impl<C: Checksum> VectoredChecksumFramer<C> {
    pub fn new(checksum_alg: C) -> Self {
        Self {
            checksum_alg,
            checksum_buf: [0; 8],
        }
    }
}

impl<C: Checksum> Framer for VectoredChecksumFramer<C> {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        let len_bytes = (payload.len() as u32).to_le_bytes();
        let checksum = self.checksum_alg.calculate(payload);
        let checksum_size = self.checksum_alg.size();
        
        // Write checksum to stack buffer
        match checksum_size {
            2 => self.checksum_buf[..2].copy_from_slice(&(checksum as u16).to_le_bytes()),
            4 => self.checksum_buf[..4].copy_from_slice(&(checksum as u32).to_le_bytes()),
            8 => self.checksum_buf[..8].copy_from_slice(&checksum.to_le_bytes()),
            _ => return Err(Error::InvalidChecksumSize),
        }
        
        // Three slices, one syscall
        let bufs = &[
            IoSlice::new(&len_bytes),
            IoSlice::new(&self.checksum_buf[..checksum_size]),
            IoSlice::new(payload),
        ];
        
        let expected_len = 4 + checksum_size + payload.len();
        match writer.write_vectored(bufs) {
            Ok(n) if n == expected_len => Ok(()),
            Ok(_) => Err(Error::PartialWrite),
            Err(e) => Err(e.into()),
        }
    }
}
```

### Phase 2: Optimized FlatBuffers Integration

```rust
/// Extended trait for FlatBuffers-aware vectored writes
pub trait FlatBufferFramer: Framer {
    /// Write with access to FlatBuffer internals for advanced optimizations
    fn frame_flatbuffer<W: Write, A: Allocator>(
        &self,
        writer: &mut W,
        builder: &FlatBufferBuilder<A>,
    ) -> Result<()> {
        // Default implementation uses standard framing
        let payload = builder.finished_data();
        self.frame_and_write(writer, payload)
    }
}

impl FlatBufferFramer for VectoredFramer {
    fn frame_flatbuffer<W: Write, A: Allocator>(
        &self,
        writer: &mut W,
        builder: &FlatBufferBuilder<A>,
    ) -> Result<()> {
        let payload = builder.finished_data();
        let len_bytes = (payload.len() as u32).to_le_bytes();
        
        // Future optimization: Access builder's internal segments
        // for scatter-gather of non-contiguous regions
        let bufs = &[
            IoSlice::new(&len_bytes),
            IoSlice::new(payload),
        ];
        
        writer.write_vectored(bufs)?;
        Ok(())
    }
}
```

### Phase 3: Batched Vectored Writes

```rust
/// High-performance batch writer using vectored I/O
pub struct BatchedVectoredWriter<W: Write> {
    writer: W,
    // Pre-allocated for typical batch sizes
    io_slices: Vec<IoSlice<'static>>,
    // Temporary storage for length prefixes and checksums
    metadata_buf: Vec<u8>,
}

impl<W: Write> BatchedVectoredWriter<W> {
    pub fn new(writer: W, capacity: usize) -> Self {
        Self {
            writer,
            io_slices: Vec::with_capacity(capacity * 3), // length + checksum + payload
            metadata_buf: Vec::with_capacity(capacity * 12), // 4 bytes length + up to 8 bytes checksum
        }
    }
    
    /// Write multiple FlatBuffers in a single vectored syscall
    pub fn write_batch<'a, A: Allocator>(
        &mut self,
        builders: &mut [FlatBufferBuilder<'a, A>],
    ) -> Result<()> {
        self.io_slices.clear();
        self.metadata_buf.clear();
        
        // Build I/O vectors for all messages
        for builder in builders.iter() {
            let payload = builder.finished_data();
            let len = payload.len() as u32;
            
            // Store length in metadata buffer
            let len_start = self.metadata_buf.len();
            self.metadata_buf.extend_from_slice(&len.to_le_bytes());
            let len_end = self.metadata_buf.len();
            
            // Add slices (these are just pointers, no copies)
            unsafe {
                // Safe because metadata_buf outlives the IoSlices
                self.io_slices.push(IoSlice::new(
                    &*(&self.metadata_buf[len_start..len_end] as *const [u8])
                ));
            }
            self.io_slices.push(IoSlice::new(payload));
        }
        
        // Single syscall for entire batch
        self.writer.write_vectored_all(&self.io_slices)?;
        Ok(())
    }
}
```

### Phase 4: Vectored Reading

```rust
/// Vectored deframing for efficient batch reads
pub struct VectoredDeframer;

impl Deframer for VectoredDeframer {
    fn read_and_deframe<R: Read>(
        &self,
        reader: &mut R,
        buffer: &mut Vec<u8>,
    ) -> Result<Option<()>> {
        // First, peek at length with minimal read
        let mut len_buf = [0u8; 4];
        match reader.read_exact(&mut len_buf) {
            Ok(_) => {},
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        
        let payload_len = u32::from_le_bytes(len_buf) as usize;
        buffer.resize(payload_len, 0);
        
        // For larger reads, consider using read_vectored for scatter-gather
        reader.read_exact(buffer)?;
        Ok(Some(()))
    }
}
```

## FlatBuffers Integration

### Leveraging FlatBuffer Structure

```rust
/// Deep integration with FlatBuffers' memory layout
pub struct FlatBufferAwareVectoredFramer {
    enable_zero_copy_strings: bool,
}

impl FlatBufferAwareVectoredFramer {
    /// Optimized for FlatBuffers with many strings/vectors
    pub fn write_with_segments<W: Write, A: Allocator>(
        &self,
        writer: &mut W,
        builder: &FlatBufferBuilder<A>,
    ) -> Result<()> {
        let payload = builder.finished_data();
        let len_bytes = (payload.len() as u32).to_le_bytes();
        
        // Future: Analyze FlatBuffer structure
        if self.enable_zero_copy_strings {
            // Detect if payload has contiguous regions that could be
            // written separately (e.g., large string tables)
            // This would require FlatBuffers API changes
        }
        
        // Current: Standard vectored write
        let bufs = &[
            IoSlice::new(&len_bytes),
            IoSlice::new(payload),
        ];
        
        writer.write_vectored(bufs)?;
        Ok(())
    }
}
```

## Performance Projections

### Microbenchmarks

```rust
#[bench]
fn bench_traditional_framing(b: &mut Bencher) {
    // Expected: ~10μs per operation
    b.iter(|| {
        writer.write_all(&len_bytes).unwrap();
        writer.write_all(&payload).unwrap();
    });
}

#[bench]
fn bench_vectored_framing(b: &mut Bencher) {
    // Expected: ~6μs per operation (40% improvement)
    b.iter(|| {
        writer.write_vectored(&[
            IoSlice::new(&len_bytes),
            IoSlice::new(&payload),
        ]).unwrap();
    });
}
```

### Real-World Impact

For a telemetry application at 100k messages/second:
- **Current**: 2M syscalls/sec, ~1-2 seconds CPU overhead
- **Vectored**: 1M syscalls/sec, ~0.5-1 second CPU overhead
- **Batch Vectored**: 10k syscalls/sec (batch=100), ~0.05-0.1 second CPU overhead

## Migration Strategy

### Phase 1: Opt-In (v3.0)

```rust
// Existing code continues to work
let framer = DefaultFramer;
let writer = StreamWriter::new(output, framer);

// Opt-in to vectored I/O
let framer = VectoredFramer;
let writer = StreamWriter::new(output, framer);
```

### Phase 2: Performance Warnings (v3.1)

```rust
#[deprecated(note = "Use VectoredFramer for better performance")]
pub struct DefaultFramer;
```

### Phase 3: Default Change (v4.0)

```rust
pub type DefaultFramer = VectoredFramer;
pub type LegacyFramer = SimpleFramer; // Renamed old implementation
```

## Future Considerations

### 1. io_uring Integration (Linux 5.1+)

```rust
#[cfg(feature = "io_uring")]
pub struct IoUringVectoredWriter {
    ring: io_uring::IoUring,
    // True zero-copy with kernel bypass
}
```

### 2. RDMA Support

For HPC/financial applications requiring ultra-low latency.

### 3. GPU Direct Storage

For ML/AI pipelines processing FlatBuffers data.

## Conclusion

Vectored I/O represents a natural evolution for `flatstream-rs`, providing significant performance improvements while maintaining its core design principles. The implementation is straightforward, backwards-compatible, and aligns perfectly with FlatBuffers' memory model.

### Key Benefits
- **50% syscall reduction** for standard use
- **95% syscall reduction** for batched operations  
- **Zero additional memory copies**
- **Natural FlatBuffers integration**
- **Backwards compatible migration path**

This enhancement positions `flatstream-rs` as the definitive high-performance FlatBuffers streaming solution for Rust.
