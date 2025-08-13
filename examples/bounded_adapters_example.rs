//! Demonstrates enforcing maximum payload sizes on both write and read paths
//! using `BoundedFramer`/`BoundedDeframer` and the fluent `.bounded()` helpers.

use flatstream::framing::{BoundedDeframer, BoundedFramer, DeframerExt, FramerExt};
use flatstream::{DefaultDeframer, DefaultFramer, Error, Result, StreamReader, StreamWriter};
use std::io::Cursor;

fn write_under_limit(bytes: &mut Vec<u8>) -> Result<()> {
    // Enforce a generous max payload length to accommodate FlatBuffer overhead
    let framer = BoundedFramer::new(DefaultFramer, 64);
    let writer = Cursor::new(bytes);
    let mut stream_writer = StreamWriter::new(writer, framer);

    // Simple mode: `&str` implements StreamSerialize in this crate
    println!(
        "[write_under_limit] Writing a small message within the configured 64-byte bound using a bounded framer"
    );
    stream_writer.write(&"hello")?; // 5 bytes, ok
    stream_writer.flush()?;
    Ok(())
}

fn write_over_limit_should_fail(bytes: &mut Vec<u8>) {
    // Use fluent composition for the framer
    let framer = DefaultFramer.bounded(4);
    let writer = Cursor::new(bytes);
    let mut stream_writer = StreamWriter::new(writer, framer);

    println!(
        "[write_over_limit_should_fail] Attempting to write a message that exceeds the configured 4-byte bound (expected error)"
    );
    let err = stream_writer.write(&"hello").unwrap_err(); // 5 bytes exceeds 4
    match err {
        Error::InvalidFrame { .. } => {}
        other => panic!("expected InvalidFrame, got {other:?}"),
    }
}

fn round_trip_with_fluent_bounded(bytes: &[u8]) -> Result<()> {
    // Same generous limit using fluent composition
    let deframer = DefaultDeframer.bounded(64);
    let mut reader = StreamReader::new(Cursor::new(bytes), deframer);

    let mut seen = 0usize;
    println!(
        "[round_trip_with_fluent_bounded] Reading all messages using a bounded deframer composed via the fluent .bounded() helper"
    );
    reader.process_all(|payload| {
        // `payload` is a borrowed slice: zero-copy
        seen += payload.len();
        Ok(())
    })?;

    println!(
        "[round_trip_with_fluent_bounded] Successfully read and processed {} total payload bytes within bounds",
        seen
    );
    Ok(())
}

fn main() -> Result<()> {
    // Happy path: write with manual BoundedFramer, read with fluent .bounded()
    println!(
        "[main] Starting bounded adapters demonstration: enforcing maximum payload sizes during write and read"
    );
    let mut bytes = Vec::new();
    write_under_limit(&mut bytes)?;
    round_trip_with_fluent_bounded(&bytes)?;

    // Failure path: attempt to write over the limit
    let mut sink = Vec::new();
    write_over_limit_should_fail(&mut sink);

    // Manual bounded deframer demonstration
    let deframer = BoundedDeframer::new(DefaultDeframer, 64);
    let mut stream_reader = StreamReader::new(Cursor::new(&bytes), deframer);
    let mut message_count = 0usize;
    println!(
        "[main] Reading messages again using an explicitly constructed bounded deframer"
    );
    stream_reader.process_all(|_payload| {
        message_count += 1;
        Ok(())
    })?;
    println!(
        "[main] Successfully read {} message(s) with the manual bounded deframer",
        message_count
    );

    Ok(())
}


