# Zero-Copy Analysis: flatstream-rs

## Overview

The entire purpose of FlatBuffers is zero-copy deserialization - once data is serialized, it can be read directly without parsing or unpacking. This document analyzes how `flatstream-rs` maintains this zero-copy principle throughout its evolution.

## What is Zero-Copy?

In the context of FlatBuffers and streaming:
- **Zero-Copy Write**: Data is serialized once into a buffer and written directly to I/O without intermediate copies
- **Zero-Copy Read**: Data is read into a buffer and accessed directly as `&[u8]` slices without copying to new allocations

## Current Implementation (v2.6): Fully Zero-Copy

### Writing: Both Modes are Zero-Copy

**Simple Mode:**
```rust
pub fn write<T: StreamSerialize>(&mut self, item: &T) -> Result<()> {
    self.builder.reset();                          // Reuse existing buffer
    item.serialize(&mut self.builder)?;            // Serialize into builder
    let payload = self.builder.finished_data();    // Get &[u8] slice - NO COPY
    self.framer.frame_and_write(&mut self.writer, payload)  // Write slice directly
}
```

**Expert Mode:**
```rust
pub fn write_finished<A: flatbuffers::Allocator>(
    &mut self,
    builder: &mut FlatBufferBuilder<A>,
) -> Result<()> {
    let payload = builder.finished_data();         // Get &[u8] slice - NO COPY
    self.framer.frame_and_write(&mut self.writer, payload)  // Write slice directly
}
```

**Key Insight**: Both modes are EQUALLY zero-copy. The performance differences come from:
- Memory management (builder sizing)
- Small call overhead in simple-mode hot loops (monomorphized call)
- Flexibility (multiple builders)

NOT from copying data!

### Reading: Perfect Zero-Copy

```rust
use flatbuffers::VerifierOptions;

// process_all API - zero-copy by design
reader.process_all(|payload: &[u8]| {
    // payload is a direct slice into reader's buffer - NO COPY
    let opts = VerifierOptions::default();
    let _event = flatbuffers::root_with_opts::<MyEvent>(&opts, payload)?;
    Ok(())
})?;

// messages() API - also zero-copy
let mut msgs = reader.messages();
while let Some(payload) = msgs.next()? {
    // Still &[u8], no allocation
}
```

## v2.5 Design: Actually WORSE for Zero-Copy

The proposed v2.5 design introduced several anti-patterns that would have compromised zero-copy behavior:

### 1. Write Batching Would Break Zero-Copy
```rust
// v2.5 proposed write_batch - would require buffering
writer.write_batch(&messages)?;  // How to batch without copying?
```

To batch messages, you'd need to either:
- Copy all messages into a contiguous buffer (breaks zero-copy)
- Use vectored I/O (not proposed in v2.5)

### 2. Complex Type-Erased API
```rust
// v2.5's Arc<RefCell<dyn Any>> approach
builder_holder: Arc<RefCell<dyn Any>>
```

This added unnecessary indirection and dynamic dispatch, moving away from the direct buffer access that enables zero-copy.

### 3. External Builder Management Misunderstood
The v2.5 design emphasized external builder management as a performance feature, but this misses the point:
- Both internal and external builders are equally zero-copy
- The benefit is flexibility, not avoiding copies

## Current Design Superiority

The current v2.6 "Hybrid API" is superior for zero-copy because:

1. **Direct Buffer Access**: No intermediate layers between builder and I/O
2. **No Artificial Batching**: Each message maintains its zero-copy path
3. **Simple Type System**: Generic parameters instead of type erasure
4. **Preserves FlatBuffers Philosophy**: The serialized format IS the wire format

## Performance Analysis Through Zero-Copy Lens

### Why Expert Mode Can Be Faster for Large Messages
It's NOT because expert mode is "more zero-copy" - both modes are equally zero-copy. The difference is:
- Less indirection (no trait object dispatch)
- Direct method call vs virtual dispatch
- But BOTH modes write the same zero-copy slice!

### Why Multiple Builders Matter
Not for zero-copy, but for memory efficiency:
```rust
// After serializing 10MB message, builder holds 10MB buffer
large_builder.reset();  // Still 10MB allocated!

// Simple mode: stuck with one 10MB builder for tiny messages
// Expert mode: can use separate small builder for tiny messages
```

## Conclusion

The zero-copy principle is fundamental to FlatBuffers and fully preserved in the current implementation:

1. **Writing**: Both simple and expert modes perform zero-copy writes via `finished_data()`
2. **Reading**: Both `process_all()` and `messages()` provide zero-copy access
3. **v2.5 Design**: Would have compromised zero-copy with batching and type erasure
4. **Current Design**: Maintains perfect zero-copy while adding flexibility

The performance differences between modes are about memory management (serialize work per-iteration) and a small monomorphized call overhead, NOT about copying data. The current implementation honors the FlatBuffers philosophy: serialize once, access everywhere, copy never.