//! Demonstrates using StreamWriter::with_builder_alloc with an explicit allocator.
//! This example uses the default allocator to illustrate the API surface; swap it
//! with your custom allocator that implements `flatbuffers::Allocator`.

use flatbuffers::FlatBufferBuilder;
use flatstream::{DefaultFramer, Result, StreamSerialize, StreamWriter};
use std::io::Cursor;

#[derive(Clone)]
struct Event(String);

impl StreamSerialize for Event {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let s = builder.create_string(&self.0);
        builder.finish(s, None);
        Ok(())
    }
}

fn main() -> Result<()> {
    println!("=== Custom Allocator Example (API surface) ===\n");

    // Replace `DefaultAllocator::default()` with your allocator that implements
    // `flatbuffers::Allocator` when available in your project.
    let alloc = flatbuffers::DefaultAllocator::default();
    let builder = FlatBufferBuilder::new_in(alloc);

    let mut out = Vec::new();
    let writer = Cursor::new(&mut out);
    let framer = DefaultFramer;

    // Provide the builder with explicit allocator to the writer (expert mode)
    let mut sw = StreamWriter::with_builder_alloc(writer, framer, builder);

    // Build and write a couple of messages
    let mut b = FlatBufferBuilder::new();
    let e1 = Event("hello alloc".into());
    b.reset();
    e1.serialize(&mut b)?;
    sw.write_finished(&mut b)?;

    let mut b2 = FlatBufferBuilder::new();
    let e2 = Event("goodbye alloc".into());
    b2.reset();
    e2.serialize(&mut b2)?;
    sw.write_finished(&mut b2)?;

    sw.flush()?;
    println!("Wrote {} framed bytes using an explicit allocator", out.len());
    Ok(())
}


