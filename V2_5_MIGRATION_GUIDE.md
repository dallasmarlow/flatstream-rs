# V2.5 Migration Guide

## Overview

The v2.5 "Processor API" introduces significant improvements to `flatstream-rs`, including zero-copy reading patterns and external `FlatBufferBuilder` management. This guide provides step-by-step instructions for migrating from v2 to v2.5.

## Breaking Changes

### 1. StreamWriter API Changes

**v2 (Internal Builder Management):**
```rust
let mut writer = StreamWriter::new(file, framer);
writer.write(&message)?; // Internal builder management
```

**v2.5 (External Builder Management):**
```rust
let mut writer = StreamWriter::new(file, framer);
let mut builder = FlatBufferBuilder::new();

// User manages the builder lifecycle
builder.reset();
message.serialize(&mut builder)?;
builder.finish(data, None);
writer.write(&mut builder)?; // External builder management
```

### 2. StreamReader API Changes

**v2 (Iterator Pattern):**
```rust
let mut reader = StreamReader::new(file, deframer);
for result in reader {
    let payload = result?; // Allocated Vec<u8>
    process_message(payload)?;
}
```

**v2.5 (Processor API - Recommended):**
```rust
let mut reader = StreamReader::new(file, deframer);
reader.process_all(|payload| {
    process_message(payload)?; // Zero-copy &[u8]
    Ok(())
})?;
```

**v2.5 (Expert Path - Manual Control):**
```rust
let mut reader = StreamReader::new(file, deframer);
let mut messages = reader.messages();
while let Some(payload) = messages.next()? {
    process_message(payload)?; // Zero-copy &[u8]
}
```

### 3. Removed Methods

The following methods have been removed in v2.5:
- `StreamWriter::write_batch()` - Use a simple `for` loop instead
- `StreamWriter::with_builder()` - No longer needed with external builder management

## Migration Steps

### Step 1: Update Dependencies

Update your `Cargo.toml` to use v2.5:

```toml
[dependencies]
flatstream-rs = "2.5.0"
```

### Step 2: Update Writing Code

**Before (v2):**
```rust
use flatstream_rs::*;
use flatbuffers::FlatBufferBuilder;

#[derive(StreamSerialize)]
struct MyMessage {
    id: u32,
    data: String,
}

fn write_messages() -> Result<()> {
    let file = File::create("data.bin")?;
    let writer = BufWriter::new(file);
    let framer = DefaultFramer;
    let mut stream_writer = StreamWriter::new(writer, framer);

    for i in 0..100 {
        let message = MyMessage {
            id: i,
            data: format!("message {}", i),
        };
        stream_writer.write(&message)?;
    }
    
    Ok(())
}
```

**After (v2.5):**
```rust
use flatstream_rs::*;
use flatbuffers::FlatBufferBuilder;

#[derive(StreamSerialize)]
struct MyMessage {
    id: u32,
    data: String,
}

fn write_messages() -> Result<()> {
    let file = File::create("data.bin")?;
    let writer = BufWriter::new(file);
    let framer = DefaultFramer;
    let mut stream_writer = StreamWriter::new(writer, framer);
    let mut builder = FlatBufferBuilder::new();

    for i in 0..100 {
        let message = MyMessage {
            id: i,
            data: format!("message {}", i),
        };
        
        // External builder management
        builder.reset();
        message.serialize(&mut builder)?;
        builder.finish(data, None);
        stream_writer.write(&mut builder)?;
    }
    
    Ok(())
}
```

### Step 3: Update Reading Code

**Before (v2):**
```rust
fn read_messages() -> Result<()> {
    let file = File::open("data.bin")?;
    let reader = BufReader::new(file);
    let deframer = DefaultDeframer;
    let mut stream_reader = StreamReader::new(reader, deframer);

    for result in stream_reader {
        let payload = result?; // Allocated Vec<u8>
        let message = process_message(payload)?;
        println!("Received: {:?}", message);
    }
    
    Ok(())
}
```

**After (v2.5) - Processor API:**
```rust
fn read_messages() -> Result<()> {
    let file = File::open("data.bin")?;
    let reader = BufReader::new(file);
    let deframer = DefaultDeframer;
    let mut stream_reader = StreamReader::new(reader, deframer);

    stream_reader.process_all(|payload| {
        // Zero-copy access to message data
        let message = process_message(payload)?;
        println!("Received: {:?}", message);
        Ok(())
    })?;
    
    Ok(())
}
```

**After (v2.5) - Expert Path:**
```rust
fn read_messages() -> Result<()> {
    let file = File::open("data.bin")?;
    let reader = BufReader::new(file);
    let deframer = DefaultDeframer;
    let mut stream_reader = StreamReader::new(reader, deframer);

    let mut messages = stream_reader.messages();
    while let Some(payload) = messages.next()? {
        // Zero-copy access to message data
        let message = process_message(payload)?;
        println!("Received: {:?}", message);
    }
    
    Ok(())
}
```

### Step 4: Update Batch Writing

**Before (v2):**
```rust
let messages = vec![/* ... */];
stream_writer.write_batch(&messages)?;
```

**After (v2.5):**
```rust
let messages = vec![/* ... */];
let mut builder = FlatBufferBuilder::new();

for message in &messages {
    builder.reset();
    message.serialize(&mut builder)?;
    builder.finish(data, None);
    stream_writer.write(&mut builder)?;
}
```

## Performance Benefits

### Zero-Copy Reading
- **v2**: Each message allocation creates a new `Vec<u8>`
- **v2.5**: Direct access to borrowed slices (`&[u8]`) from internal buffers

### External Builder Management
- **v2**: Internal builder management with hidden allocations
- **v2.5**: User-controlled builder lifecycle for optimal memory usage

### Performance Improvements
- **Reading**: Up to 4.55x faster
- **Write-Read Cycles**: Up to 2.99x faster
- **Memory Usage**: Eliminated per-message allocations

## Best Practices

### 1. Choose the Right Reading Pattern

**Use Processor API for:**
- Simple message processing
- High-performance bulk operations
- When you don't need manual control

```rust
reader.process_all(|payload| {
    process_message(payload)?;
    Ok(())
})?;
```

**Use Expert Path for:**
- Manual iteration control
- Early termination
- Complex processing logic

```rust
let mut messages = reader.messages();
while let Some(payload) = messages.next()? {
    if should_stop_processing(payload)? {
        break;
    }
    process_message(payload)?;
}
```

### 2. Optimize Builder Usage

**Reuse the same builder:**
```rust
let mut builder = FlatBufferBuilder::new();
for message in messages {
    builder.reset(); // Reset instead of creating new
    message.serialize(&mut builder)?;
    builder.finish(data, None);
    writer.write(&mut builder)?;
}
```

### 3. Handle Errors Properly

**Processor API error handling:**
```rust
let result = reader.process_all(|payload| {
    process_message(payload)?;
    Ok(())
});

match result {
    Ok(()) => println!("Processing completed successfully"),
    Err(e) => eprintln!("Processing failed: {}", e),
}
```

**Expert Path error handling:**
```rust
let mut messages = reader.messages();
while let Some(payload) = messages.next()? {
    process_message(payload)?;
}
```

## Common Migration Patterns

### Pattern 1: Simple Message Processing

**v2:**
```rust
for result in reader {
    let payload = result?;
    let message = deserialize_message(payload)?;
    handle_message(message)?;
}
```

**v2.5:**
```rust
reader.process_all(|payload| {
    let message = deserialize_message(payload)?;
    handle_message(message)?;
    Ok(())
})?;
```

### Pattern 2: Message Counting

**v2:**
```rust
let mut count = 0;
for result in reader {
    let _payload = result?;
    count += 1;
}
```

**v2.5:**
```rust
let mut count = 0;
reader.process_all(|_payload| {
    count += 1;
    Ok(())
})?;
```

### Pattern 3: Early Termination

**v2:**
```rust
for result in reader {
    let payload = result?;
    if should_stop(payload)? {
        break;
    }
    process_message(payload)?;
}
```

**v2.5:**
```rust
let mut messages = reader.messages();
while let Some(payload) = messages.next()? {
    if should_stop(payload)? {
        break;
    }
    process_message(payload)?;
}
```

## Troubleshooting

### Common Issues

**Issue: "StreamReader is not an iterator"**
- **Solution**: Replace `for result in reader` with `reader.process_all()` or `reader.messages().next()`

**Issue: "expected &mut FlatBufferBuilder, found &T"**
- **Solution**: Use external builder management as shown in the migration examples

**Issue: "cannot find function write_batch"**
- **Solution**: Replace with a simple `for` loop using external builder management

### Performance Tips

1. **Reuse FlatBufferBuilder**: Create one builder and reset it for each message
2. **Use Processor API**: Prefer `process_all()` over `messages().next()` for simple cases
3. **Avoid unnecessary allocations**: The zero-copy design eliminates per-message allocations
4. **Profile your code**: Use `cargo bench` to measure performance improvements

## Testing Your Migration

### 1. Run Your Tests
```bash
cargo test
```

### 2. Benchmark Performance
```bash
cargo bench
```

### 3. Check for Warnings
```bash
cargo check
```

## Support

If you encounter issues during migration:

1. **Check the examples**: See `examples/` directory for v2.5 usage patterns
2. **Review the API documentation**: Updated documentation reflects v2.5 changes
3. **Run the integration tests**: Verify your usage patterns against the test suite

## Conclusion

The v2.5 migration provides significant performance improvements and better API design. While the changes require code updates, the benefits in terms of performance, memory usage, and API clarity make the migration worthwhile.

The key principles to remember:
- **External builder management** for optimal memory usage
- **Zero-copy reading** for maximum performance
- **Processor API** for simple cases, **Expert Path** for complex control
- **Proper error handling** for robust applications

Happy migrating! 🚀 