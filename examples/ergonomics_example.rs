//! Demonstrates the ergonomic helpers on StreamReader and StreamWriter.
//! Example purpose: Show accessors, capacity management (with_capacity, reserve),
//! and zero-copy process_all() usage in a concise flow.

use flatbuffers::FlatBufferBuilder;
use flatstream::{DefaultDeframer, DefaultFramer, Result, StreamReader, StreamWriter};
use std::io::Cursor;

fn main() -> Result<()> {
    // Writer ergonomics
    let mut output_bytes = Vec::new();
    let writer = Cursor::new(&mut output_bytes);
    let framer = DefaultFramer;

    // Pre-sizing with a provided builder
    let builder = FlatBufferBuilder::new();
    let mut stream_writer = StreamWriter::with_builder(writer, framer, builder);
    // Strategy access is read-only; sink access requires `into_inner`.
    let _framer_ref = stream_writer.framer();

    // Expert mode write
    let mut b = FlatBufferBuilder::new();
    let s = b.create_string("hello ergonomics");
    b.finish(s, None);
    println!(
        "[writer] Writing a finished FlatBuffer with a pre-constructed builder for predictable capacity"
    );
    stream_writer.write_finished(&mut b)?;
    stream_writer.flush()?;

    // Reader ergonomics
    let reader = Cursor::new(stream_writer.into_inner().into_inner());
    let deframer = DefaultDeframer::new();
    let mut stream_reader = StreamReader::with_capacity(reader, deframer, 1024);
    assert!(stream_reader.buffer_capacity() >= 1024);

    // Ensure capacity using reserve
    println!(
        "[reader] Reserving capacity to at least 2048 bytes to avoid future reallocations during reads"
    );
    stream_reader.reserve(2048);
    assert!(stream_reader.buffer_capacity() >= 2048);

    // Strategy access is read-only; source access requires `into_inner`.
    let _deframer_ref = stream_reader.deframer();

    // Process messages
    let capacity_before = stream_reader.buffer_capacity();
    let mut message_count = 0usize;
    println!("[reader] Processing all messages with zero-copy payload slices");
    stream_reader.process_all(|payload| {
        println!("[reader] Received a payload of {} bytes", payload.len());
        assert_eq!(
            flatbuffers::root::<&str>(payload).expect("payload is a FlatBuffers string"),
            "hello ergonomics",
            "the payload must survive the round trip unchanged"
        );
        message_count += 1;
        Ok(())
    })?;

    assert_eq!(
        message_count, 1,
        "expected exactly one message; a silent zero would otherwise pass unnoticed"
    );
    // This is what `reserve` was for: a frame that fits the reserved buffer
    // must not trigger a reallocation.
    assert_eq!(
        stream_reader.buffer_capacity(),
        capacity_before,
        "reading a frame smaller than the reserved capacity should not have reallocated"
    );
    println!("[reader] Completed reading {message_count} message(s) with no reallocation");

    // Take back the inner reader
    let _inner_reader = stream_reader.into_inner();

    Ok(())
}
