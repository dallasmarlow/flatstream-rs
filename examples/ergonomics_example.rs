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
    // Accessors
    let _writer_ref = stream_writer.get_ref();
    let _writer_mut = stream_writer.get_mut();
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
    let deframer = DefaultDeframer;
    let mut stream_reader = StreamReader::with_capacity(reader, deframer, 1024);
    assert!(stream_reader.buffer_capacity() >= 1024);

    // Ensure capacity using reserve
    println!(
        "[reader] Reserving capacity to at least 2048 bytes to avoid future reallocations during reads"
    );
    stream_reader.reserve(2048);
    assert!(stream_reader.buffer_capacity() >= 2048);

    // Accessors
    let _reader_ref = stream_reader.get_ref();
    let _reader_mut = stream_reader.get_mut();
    let _deframer_ref = stream_reader.deframer();

    // Process messages
    let mut message_count = 0usize;
    println!("[reader] Processing all messages with zero-copy payload slices");
    stream_reader.process_all(|payload| {
        println!("[reader] Received a payload of {} bytes", payload.len());
        message_count += 1;
        Ok(())
    })?;
    println!("[reader] Completed reading {message_count} message(s)");

    // Take back the inner reader
    let _inner_reader = stream_reader.into_inner();

    Ok(())
}
