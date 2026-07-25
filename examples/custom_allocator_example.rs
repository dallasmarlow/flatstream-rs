//! Demonstrates using StreamWriter::with_builder_alloc with an explicit allocator.
//! This example uses the default allocator to illustrate the API surface; swap it
//! with your custom allocator that implements `flatbuffers::Allocator`.

use flatbuffers::FlatBufferBuilder;
use flatstream::{
    DefaultDeframer, DefaultFramer, Result, StreamReader, StreamSerialize, StreamWriter,
};
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

    // Provide the builder with explicit allocator to the writer.
    let mut sw = StreamWriter::with_builder_alloc(writer, framer, builder);

    // Simple mode works with any allocator: this write serializes through the
    // internal, explicit-allocator builder provided above.
    let e1 = Event("hello alloc".into());
    sw.write(&e1)?;

    // Expert mode also composes: bring an external builder (with its own
    // explicit allocator) and hand over finished messages.
    let alloc2 = flatbuffers::DefaultAllocator::default();
    let mut b2 = FlatBufferBuilder::new_in(alloc2);
    let e2 = Event("goodbye alloc".into());
    b2.reset();
    e2.serialize(&mut b2)?;
    sw.write_finished(&mut b2)?;

    sw.flush()?;
    let reported = sw.bytes_written();
    drop(sw);

    // The point of the example is that the allocator choice is invisible on the
    // wire: both paths must produce ordinary frames a plain reader can consume.
    assert_eq!(
        reported as usize,
        out.len(),
        "bytes_written() must agree with what actually landed in the sink"
    );

    let mut reader = StreamReader::new(Cursor::new(&out), DefaultDeframer::new());
    let mut round_tripped = Vec::new();
    reader.process_all(|payload| {
        round_tripped.push(
            flatbuffers::root::<&str>(payload)
                .expect("payload is a valid FlatBuffers string")
                .to_string(),
        );
        Ok(())
    })?;
    assert_eq!(
        round_tripped,
        vec!["hello alloc".to_string(), "goodbye alloc".to_string()],
        "both the simple-mode and expert-mode writes must round-trip in order"
    );

    println!(
        "Wrote {} framed bytes using an explicit allocator; {} messages round-tripped",
        out.len(),
        round_tripped.len()
    );
    Ok(())
}
