# FlatStream-RS v2.5 Migration Guide

## Overview

This guide provides a comprehensive migration path from `flatstream-rs` v2 to v2.5. The v2.5 "Processor API" design introduces breaking changes that are necessary to achieve the performance and safety goals of making the "fast path" the only path.

## Breaking Changes Summary

| Component | v2 API | v2.5 API | Impact |
|-----------|--------|----------|--------|
| **StreamWriter** | Internal builder management | External builder management | High |
| **StreamWriter** | `write_batch()` method | Removed (use for loops) | Medium |
| **StreamReader** | Iterator pattern | `process_all()` and `messages()` | High |
| **StreamReader** | Allocating `Vec<u8>` returns | Zero-copy `&[u8]` slices | High |

## StreamWriter Migration

### **Core Change: External Builder Management**

The v2.5 design removes internal builder management from `StreamWriter`, requiring users to manage the `FlatBufferBuilder` lifecycle explicitly. This change enables zero-allocation writes and arena allocation support.

#### **v2 API (Deprecated)**
```rust
use flatstream_rs::{StreamWriter, DefaultFramer};
use std::fs::File;

// Internal builder management
let file = File::create("data.bin")?;
let mut writer = StreamWriter::new(file, DefaultFramer);

// Write with internal builder
let message = "Hello, World!";
writer.write(&message)?;

// Batch writing
let messages = vec!["msg1", "msg2", "msg3"];
writer.write_batch(&messages)?;
```

#### **v2.5 API (New)**
```rust
use flatstream_rs::{StreamWriter, DefaultFramer};
use flatbuffers::FlatBufferBuilder;
use std::fs::File;

// External builder management
let file = File::create("data.bin")?;
let mut writer = StreamWriter::new(file, DefaultFramer);
let mut builder = FlatBufferBuilder::new();

// Write with external builder
let message = "Hello, World!";
let data = builder.create_string(message);
builder.finish(data, None);
writer.write(&mut builder)?;

// Batch writing (explicit for loop)
let messages = vec!["msg1", "msg2", "msg3"];
for message in &messages {
    builder.reset(); // Reuse the builder
    let data = builder.create_string(message);
    builder.finish(data, None);
    writer.write(&mut builder)?;
}
```

### **Benefits of External Builder Management**

1. **Zero-Allocation Writes**: Builder reuse eliminates allocations
2. **Arena Allocation**: Support for custom allocators like `bumpalo`
3. **Explicit Control**: Clear lifecycle management
4. **Performance**: Better compiler optimizations

#### **Arena Allocation Example**
```rust
use flatstream_rs::{StreamWriter, DefaultFramer};
use flatbuffers::FlatBufferBuilder;
use bumpalo::Bump;

let file = File::create("data.bin")?;
let mut writer = StreamWriter::new(file, DefaultFramer);

// Arena allocation for extreme performance
let bump = Bump::new();
let mut builder = FlatBufferBuilder::new_in_bump_allocator(&bump);

for _ in 0..1000 {
    // Sample data in real-time
    let live_data = sample_shared_memory();
    
    // Build message with arena allocation
    let args = TelemetryEventArgs { /* ... */ };
    let event = TelemetryEvent::create(&mut builder, &args);
    builder.finish(event, None);
    
    // Zero-allocation write
    writer.write(&mut builder)?;
}
```

## StreamReader Migration

### **Core Change: Zero-Copy Processing**

The v2.5 design replaces the allocating Iterator pattern with zero-copy processing methods that guarantee memory safety through the borrow checker.

#### **v2 API (Deprecated)**
```rust
use flatstream_rs::{StreamReader, DefaultDeframer};
use std::fs::File;

let file = File::open("data.bin")?;
let reader = StreamReader::new(file, DefaultDeframer);

// Allocating Iterator pattern
for result in reader {
    let payload: Vec<u8> = result?; // Allocation on each iteration
    let event = flatbuffers::get_root::<telemetry::Event>(&payload)?;
    process_event(event);
}
```

#### **v2.5 API (New)**

##### **Simple Path: `process_all()`**
```rust
use flatstream_rs::{StreamReader, DefaultDeframer};
use std::fs::File;

let file = File::open("data.bin")?;
let mut reader = StreamReader::new(file, DefaultDeframer);

// Zero-copy processing with closure
reader.process_all(|payload: &[u8]| {
    let event = flatbuffers::get_root::<telemetry::Event>(payload)?;
    process_event(event);
    Ok(()) // Return Ok to continue, Err to stop
})?;
```

##### **Expert Path: `messages()`**
```rust
use flatstream_rs::{StreamReader, DefaultDeframer};
use std::fs::File;

let file = File::open("data.bin")?;
let mut reader = StreamReader::new(file, DefaultDeframer);
let mut messages = reader.messages(); // Create processor

// User-controlled loop with zero-copy slices
while let Some(payload) = messages.next()? {
    let event = flatbuffers::get_root::<telemetry::Event>(payload)?;
    process_event(event);
}
```

### **Benefits of Zero-Copy Processing**

1. **Memory Efficiency**: No allocations during processing
2. **Performance**: Reduced memory pressure and cache misses
3. **Safety**: Borrow checker guarantees slice validity
4. **Flexibility**: Both simple and expert usage patterns

## Migration Patterns

### **Pattern 1: Simple Message Processing**

#### **v2 (Deprecated)**
```rust
let mut writer = StreamWriter::new(file, DefaultFramer);
let reader = StreamReader::new(file, DefaultDeframer);

// Write
writer.write(&message)?;

// Read
for result in reader {
    let payload = result?;
    process_message(&payload);
}
```

#### **v2.5 (New)**
```rust
let mut builder = FlatBufferBuilder::new();
let mut writer = StreamWriter::new(file, DefaultFramer);
let mut reader = StreamReader::new(file, DefaultDeframer);

// Write
let data = builder.create_string(&message);
builder.finish(data, None);
writer.write(&mut builder)?;

// Read
reader.process_all(|payload| {
    process_message(payload);
    Ok(())
})?;
```

### **Pattern 2: High-Frequency Telemetry**

#### **v2 (Deprecated)**
```rust
let mut writer = StreamWriter::new(file, DefaultFramer);

// Batch writing
let events = collect_telemetry_events();
writer.write_batch(&events)?;
```

#### **v2.5 (New)**
```rust
let mut builder = FlatBufferBuilder::new();
let mut writer = StreamWriter::new(file, DefaultFramer);

// Explicit hot loop
for event in collect_telemetry_events() {
    builder.reset();
    let data = builder.create_string(&event);
    builder.finish(data, None);
    writer.write(&mut builder)?;
}
```

### **Pattern 3: Advanced Processing with Chunking**

#### **v2 (Deprecated)**
```rust
let reader = StreamReader::new(file, DefaultDeframer);
let mut chunk = Vec::new();

for result in reader {
    let payload = result?;
    chunk.push(payload);
    
    if chunk.len() >= 100 {
        process_chunk(&chunk);
        chunk.clear();
    }
}
```

#### **v2.5 (New)**
```rust
let mut reader = StreamReader::new(file, DefaultDeframer);
let mut messages = reader.messages();
let mut chunk = Vec::new();

while let Some(payload) = messages.next()? {
    chunk.push(payload.to_vec()); // Convert to owned if needed
    
    if chunk.len() >= 100 {
        process_chunk(&chunk);
        chunk.clear();
    }
}
```

## Performance Considerations

### **Builder Reuse**
Always reuse the `FlatBufferBuilder` when possible to avoid allocations:

```rust
let mut builder = FlatBufferBuilder::new();
let mut writer = StreamWriter::new(file, DefaultFramer);

for message in messages {
    builder.reset(); // Reuse the builder
    let data = builder.create_string(message);
    builder.finish(data, None);
    writer.write(&mut builder)?;
}
```

### **Zero-Copy Processing**
Prefer `process_all()` for simple processing and `messages()` for advanced control:

```rust
// Simple processing
reader.process_all(|payload| {
    process_event(payload);
    Ok(())
})?;

// Advanced control
let mut messages = reader.messages();
while let Some(payload) = messages.next()? {
    if should_stop_processing(payload) {
        break;
    }
    process_event(payload);
}
```

## Common Migration Issues

### **Issue 1: Builder Lifecycle Management**
**Problem**: Forgetting to call `builder.reset()` between writes
**Solution**: Always reset the builder or create a new one for each message

### **Issue 2: Payload Ownership**
**Problem**: Trying to store zero-copy slices beyond their lifetime
**Solution**: Convert to owned data if needed: `payload.to_vec()`

### **Issue 3: Error Handling**
**Problem**: Not handling errors in `process_all()` closure
**Solution**: Always return `Result<(), Error>` from the closure

## Testing Your Migration

### **Unit Test Migration**
```rust
#[test]
fn test_write_read_cycle() {
    let mut buffer = Vec::new();
    let mut builder = FlatBufferBuilder::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
    
    // Write
    let data = builder.create_string("test");
    builder.finish(data, None);
    writer.write(&mut builder).unwrap();
    
    // Read
    let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
    let mut found = false;
    reader.process_all(|payload| {
        assert_eq!(payload, b"test");
        found = true;
        Ok(())
    }).unwrap();
    
    assert!(found);
}
```

## Conclusion

The v2.5 migration provides significant benefits in performance, safety, and developer experience. While the breaking changes require code updates, the new API patterns are more explicit, safer, and better suited for high-performance applications.

The key principles to remember:
1. **External builder management** enables zero-allocation writes
2. **Zero-copy processing** eliminates read allocations
3. **Explicit control** provides better performance and safety
4. **Closure-based processing** is idiomatic Rust

For assistance with migration, refer to the examples in the `examples/` directory and the comprehensive test suite in `tests/`. 