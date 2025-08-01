Design Document: flatstream-rs v2.5 - The Processor API
Version: 1.0
Status: Proposed
Author: Dallas Marlow
1. Intent and Motivation
This document outlines the final proposed design for flatstream-rs v2.5. The development process has revealed that the library's sole production use case is for a high-frequency, wide-format telemetry agent. This singular focus requires an API that is not just performant, but one that makes the zero-copy, zero-allocation "fast path" the only path, guiding all developers to the correct usage pattern without fail.
The core business need is a low-level library that is unapologetically optimized for performance and safety, while remaining simple enough for junior engineers to use correctly. This design achieves that goal by refactoring the API around a "Processor" pattern, where the library provides a highly-optimized engine, and the user provides the business logic to be executed.
2. The Refinement Process: From v2.1 to v2.5
Our design discussion has been invaluable in reaching this point. We have evolved through several proposals:
v2.1 "Polish" Plan: This plan focused on incremental improvements like sized checksums and opt-in arena allocation. This was a good start, but it left the "easy, slow path" (Iterator, internal builder) as the default, which was a risk for our production use case.
v2.2 "Pure" Plan: This plan made the correct technical decision to remove the allocating paths but resulted in a less ergonomic API, forcing users into low-level patterns for every task.
v2.5 "Processor" Plan (This Proposal): This design synthesizes the best of both worlds. It adopts the pure, high-performance engine from the v2.2 plan but wraps it in a simple, safe, and elegant functional abstraction that is both easier to use and impossible to misuse from a performance perspective.
3. Core Design: The Processor API
This design refactors the StreamReader and StreamWriter into pure, focused engines. The primary user interaction is now through providing closures (pluggable functions) or managing an external builder.
3.1. The StreamWriter: A Pure I/O Engine
The StreamWriter will be simplified to its essential function: framing and writing bytes. It will no longer manage a FlatBufferBuilder internally.
Implementation:
The builder field is removed from the StreamWriter struct.
The new() constructor now only takes a writer and a framer.
The write() method is modified to accept a mutable reference to a FlatBufferBuilder as a parameter.
The old write_batch() method is removed, as a simple for loop in user code is now the more flexible and explicit pattern for batching.
User Experience (UX):
Rust
use flatbuffers::FlatBufferBuilder;

// 1. User creates and manages the builder's lifecycle.
// This naturally encourages reuse and enables arena allocation.
let mut builder = FlatBufferBuilder::new();
let mut writer = StreamWriter::new(file, DefaultFramer);

// 2. The user has full control of the "busy loop" for real-time generation.
for _ in 0..iterations {
    // Sample data in real-time
    let live_data = sample_shared_memory();

    // Build the message inside the loop
    let args = TelemetryEventArgs { /* ... */ };
    let event = TelemetryEvent::create(&mut builder, &args);
    builder.finish(event, None);

    // 3. The write call is explicit and performant.
    // This signature should take the finished builder directly.
    writer.write(&mut builder)?;
}


3.2. The StreamReader: A Safe, Zero-Copy Processor
The StreamReader will be the primary mechanism for safe, zero-copy processing. It will provide two distinct methods for reading, catering to both simple and advanced use cases.
The Simple Path: process_all()
This method will be the default way to read a stream. It abstracts away the read loop entirely.
Implementation: The StreamReader will execute its internal, high-performance while let loop and pass a borrowed &[u8] slice to a user-provided closure for each message.
User Experience (UX):
Rust
let mut reader = StreamReader::new(file, DefaultDeframer);

// User provides their logic; the library guarantees zero-copy.
reader.process_all(|payload: &[u8]| {
    let event = flatbuffers::get_root::<telemetry::Event>(payload)?;
    // ... process the event ...
    Ok(()) // Return Ok to continue, or Err to stop.
})?;


The Expert Path: messages()
This method provides more control for advanced use cases like chunking or early exits.
Implementation: The reader.messages() method will return a temporary "processor" struct (Messages<'a>) that borrows the reader. This processor struct will have a next() method that returns the next zero-copy slice. The Rust borrow checker will ensure the slice cannot be misused.
User Experience (UX):
Rust
let mut reader = StreamReader::new(file, DefaultDeframer);
let mut messages = reader.messages(); // Create the processor
let mut chunk = Vec::new();

// User controls the loop for custom logic.
while let Some(payload) = messages.next()? {
    // 'payload' is still a guaranteed zero-copy slice.
    chunk.push(process_payload(payload));
    if chunk.len() >= 100 {
        process_chunk(&mut chunk);
    }
}


4. How This Design Meets Our Goals
This refined plan directly accomplishes all of our stated design goals:
Reliable / High-Performance Format: ✅ Preserved. The on-disk format and core framing logic are unchanged.
Bullet-Proof Zero-Copy: ✅ Achieved. The process_all method and the messages().next() API both make zero-copy reads the only possible path, guaranteed by the compiler. The StreamWriter API makes zero-allocation writes the default pattern.
Pluggability and Extensibility: ✅ Maintained. The Framer, Checksum, and StreamSerialize traits are untouched, allowing future extension.
Elegance and Simplicity: ✅ Achieved. The API is now more focused. The "processor" pattern is a simple, powerful abstraction that hides complexity.
Minimal Lines of Code: ✅ Achieved. This plan results in a net reduction of code within the library's core by removing the Iterator implementation and simplifying the StreamWriter.
Idiomatic Rust: ✅ Achieved. Using closures to provide logic and leveraging the borrow checker to guarantee memory safety is a core, idiomatic pattern in high-performance Rust.
Merit for the Immediate Use Case: ✅ Perfected. The StreamWriter API is now perfectly suited for the agent's "sample-build-emit" hot loop. The StreamReader provides a safe and simple way to build offline analysis tools. The design remains generic over any Read/Write sink, ensuring future flexibility for streaming to shared memory or network sockets.

