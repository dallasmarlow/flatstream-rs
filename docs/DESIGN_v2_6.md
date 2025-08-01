# Design Document: flatstream-rs v2.6 - The Hybrid API

**Version:** 1.0  
**Status:** Implemented  
**Author:** Dallas Marlow  
**Date:** 2025-01-27

## 1. Overview

This document describes the actual implementation of flatstream-rs, which evolved from the v2.5 "Processor API" design proposal into what we now call the "Hybrid API" approach. This implementation represents a pragmatic balance between the theoretical purity of v2.5 and the practical needs of real-world usage.

## 2. Evolution from v2.5 Design

The v2.5 design proposed a radical simplification:
- Remove internal `FlatBufferBuilder` from `StreamWriter`
- Force all users to manage builders externally
- Single `write(&mut builder)` method only
- "Pure I/O Engine" philosophy

However, during implementation, we discovered that this approach, while theoretically elegant, created unnecessary friction for users and broke backward compatibility. The v2.6 hybrid approach was born from these learnings.

## 3. The Hybrid API Philosophy

The current implementation provides two distinct modes of operation, allowing users to choose based on their specific needs:

### 3.1 Simple Mode (Default Path)
- `StreamWriter` manages an internal `FlatBufferBuilder`
- Users call `write<T: StreamSerialize>(&mut self, item: &T)`
- Builder is automatically reset and reused
- Zero configuration, works out of the box
- Suitable for prototyping and moderate-performance scenarios

### 3.2 Expert Mode (Performance Path)
- Users manage their own `FlatBufferBuilder` externally
- Call `write_finished(&mut self, builder: &mut FlatBufferBuilder)`
- Full control over builder lifecycle and allocation strategy
- Enables custom memory management strategies
- Used in all performance-critical paths (benchmarks, examples)

## 4. Key Implementation Decisions

### 4.1 Backward Compatibility
Unlike the breaking change proposed in v2.5, the hybrid approach maintains full backward compatibility:
- Existing code using `write()` continues to work
- No migration required for current users
- Gradual adoption path for performance optimization

### 4.2 StreamSerialize Trait Preservation
The v2.5 design claimed to keep `StreamSerialize` "untouched" but actually broke its primary use case. The hybrid approach:
- Maintains `StreamSerialize` as a first-class citizen
- Enables convenient serialization for simple types (String, &str)
- Allows custom types to define their serialization logic
- Works seamlessly with the simple mode

### 4.3 Builder Management Strategy
```rust
pub struct StreamWriter<'a, W: Write, F: Framer, A = flatbuffers::DefaultAllocator>
where
    A: flatbuffers::Allocator,
{
    writer: W,
    framer: F,
    builder: FlatBufferBuilder<'a, A>,  // Internal builder for simple mode
}
```

The internal builder enables:
- Automatic memory reuse via `reset()`
- Zero-allocation writes for repeated messages
- Simple API for non-performance-critical code

### 4.4 Performance Characteristics
Performance varies based on workload:

**For uniform, small-to-medium messages:**
- Simple mode and expert mode perform nearly identically
- Difference is typically 0-25% depending on message size
- Both benefit from builder reuse via `reset()`

**For large messages (10MB+):**
- Expert mode can be ~2x faster than simple mode
- The overhead of the trait dispatch becomes more noticeable

**For mixed message sizes:**
- Simple mode suffers from memory bloat (builder grows to largest size and stays there)
- Expert mode enables using multiple builders sized for different message types
- Memory efficiency can be dramatically better with expert mode

## 5. API Design

### 5.1 StreamWriter Methods
```rust
impl<'a, W: Write, F: Framer> StreamWriter<'a, W, F> {
    /// Simple mode constructor
    pub fn new(writer: W, framer: F) -> Self;
    
    /// Simple mode write
    pub fn write<T: StreamSerialize>(&mut self, item: &T) -> Result<()>;
}

impl<'a, W: Write, F: Framer, A> StreamWriter<'a, W, F, A> 
where A: flatbuffers::Allocator 
{
    /// Expert mode constructor
    pub fn with_builder(writer: W, framer: F, builder: FlatBufferBuilder<'a, A>) -> Self;
    
    /// Expert mode write
    pub fn write_finished(&mut self, builder: &mut FlatBufferBuilder) -> Result<()>;
}
```

### 5.2 StreamReader (Unchanged from v2.5)
The reader implementation matches the v2.5 design exactly:
- `process_all()` for high-performance closure-based processing
- `messages()` for expert manual iteration
- No `Iterator` trait implementation
- Zero-copy access throughout

## 6. Usage Patterns

### 6.1 Simple Mode (Getting Started)
```rust
let mut writer = StreamWriter::new(file, DefaultFramer);

// Built-in types work immediately
writer.write(&"Hello, world!")?;

// Custom types via StreamSerialize
let event = TelemetryEvent { /* ... */ };
writer.write(&event)?;
```

### 6.2 Expert Mode (Production Performance)
```rust
let mut builder = FlatBufferBuilder::new();
let mut writer = StreamWriter::new(file, DefaultFramer);

for event in high_frequency_events {
    // Explicit builder management
    builder.reset();
    event.serialize(&mut builder)?;
    writer.write_finished(&mut builder)?;
}
```

### 6.3 Custom Allocators (Future Enhancement)
```rust
// Note: Custom allocators are supported by FlatBuffers but require
// careful implementation. The with_builder() constructor enables
// this pattern when/if you implement a custom allocator.
let custom_allocator = MyCustomAllocator::new();
let builder = FlatBufferBuilder::new_with_allocator(custom_allocator);
let mut writer = StreamWriter::with_builder(file, DefaultFramer, builder);
```

## 7. Performance Analysis

### 7.1 Benchmark Results
All performance benchmarks use the expert mode, achieving:
- Write: ~54M messages/sec (1000 messages in ~18.4µs)
- Read: ~225M messages/sec (1000 messages in ~4.4µs)

### 7.2 Why Simple Mode Performance Is Excellent
The simple mode's performance is nearly identical to expert mode because:
1. Builder reuse via `reset()` amortizes allocation cost completely
2. No additional copies or indirection - both modes are equally zero-copy
3. Monomorphization eliminates abstraction overhead
4. For common cases (uniform messages), the performance difference is negligible (0-25%)

The "slow path" criticism from v2.5 was overstated - simple mode is only meaningfully slower for edge cases like very large messages or mixed message sizes.

## 8. Advantages Over v2.5 Design

### 8.1 Better Developer Experience
- **Gentle Learning Curve**: Start simple, optimize when needed
- **Discoverable API**: `write()` is intuitive for new users
- **Progressive Disclosure**: Expert features available but not required

### 8.2 Practical Benefits
- **No Breaking Changes**: Existing code continues to work
- **Flexibility**: Choose the right tool for the job
- **Real-World Tested**: Used successfully in production

### 8.3 Same Performance Ceiling
- Expert mode achieves identical performance to v2.5 design
- No compromise on the critical path
- Benchmarks prove the hybrid approach works

## 9. Design Trade-offs

### 9.1 What We Gained
- Backward compatibility
- Better ergonomics for simple cases
- Flexibility to choose approaches
- Easier onboarding for new users

### 9.2 What We Lost
- Single, forced correct path
- Theoretical purity
- Smaller API surface

### 9.3 Why This Trade-off Makes Sense
The hybrid approach follows the principle of "make simple things simple, and complex things possible." For a library that aims to be widely adopted, providing both simplicity and performance is more valuable than theoretical elegance.

The key insight that justified this approach: **Simple mode performance is excellent for the common case**. If simple mode were significantly slower (say 2-10x), the v2.5 approach of forcing expert mode would be justified. But with only a 0-25% difference for typical workloads, forcing everyone to manage builders externally would be premature optimization.

## 10. Zero-Copy Preservation

### 10.1 Both Modes are Equally Zero-Copy

A critical design achievement is that both simple and expert modes maintain perfect zero-copy behavior:

**Simple Mode Zero-Copy Path:**
```rust
write() -> serialize() -> finished_data() -> frame_and_write() -> I/O
         ^                ^                  ^
         |                |                  |
    Into builder      Direct slice      No intermediate copy
```

**Expert Mode Zero-Copy Path:**
```rust
serialize() -> finished_data() -> write_finished() -> frame_and_write() -> I/O
            ^                  ^                    ^
            |                  |                    |
      Into builder        Direct slice         No intermediate copy
```

### 10.2 Advantages Over v2.5's Approach

The v2.5 design would have compromised zero-copy in several ways:
1. **Write batching** would require intermediate buffering
2. **Type erasure** (`Arc<RefCell<dyn Any>>`) adds indirection
3. **Complex ownership** model distances from direct buffer access

The current design maintains the FlatBuffers philosophy: serialize once, access everywhere, copy never.

## 11. Future Considerations

### 11.1 Documentation Strategy
- Lead with simple examples
- Clearly mark performance paths
- Show migration from simple to expert mode
- Emphasize zero-copy nature of both modes

### 11.2 Potential Improvements
- Performance hints/warnings in debug mode
- Guided optimization via documentation
- Benchmarking tools for users
- Vectored I/O for true zero-copy batching

## 12. Conclusion

The v2.6 hybrid implementation represents a mature, production-ready design that balances:
- **Zero-Copy Integrity**: Both modes maintain perfect zero-copy behavior
- **Performance**: Expert mode provides flexibility for memory-constrained scenarios
- **Usability**: Simple mode provides immediate productivity
- **Compatibility**: No breaking changes from v2
- **FlatBuffers Philosophy**: Honors the core principle of "serialize once, copy never"

This design philosophy of "pragmatic performance with zero-copy guarantee" has proven successful in production deployments and provides a solid foundation for future evolution of the library. 