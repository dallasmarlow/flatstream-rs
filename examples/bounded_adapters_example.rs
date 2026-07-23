// Example purpose: Shows how to enforce frame/payload limits — the bounded framer
// adapter on the write path, the deframers' built-in `max_frame_len` on the read
// path — and what errors to expect when limits are exceeded (InvalidFrame with context).
//! Demonstrates enforcing maximum payload sizes on both write and read paths.
//! Reading defaults to the wire format's ~4 GiB ceiling (the full u32 range);
//! `with_max_frame_len` tightens the bound for untrusted input.

use flatstream::framing::{BoundedFramer, FramerExt};
use flatstream::{DefaultDeframer, DefaultFramer, ErrorKind, Result, StreamReader, StreamWriter};
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
    match err.into_kind() {
        ErrorKind::InvalidFrame { .. } => {}
        other => panic!("expected InvalidFrame, got {other:?}"),
    }
}

fn round_trip_with_tight_bound(bytes: &[u8]) -> Result<()> {
    // Same generous limit configured directly on the deframer
    let deframer = DefaultDeframer::new().with_max_frame_len(64);
    let mut reader = StreamReader::new(Cursor::new(bytes), deframer);

    let mut seen = 0usize;
    println!(
        "[round_trip_with_tight_bound] Reading all messages with the deframer's max_frame_len tightened to 64 bytes"
    );
    reader.process_all(|payload| {
        // `payload` is a borrowed slice: zero-copy
        seen += payload.len();
        Ok(())
    })?;

    println!(
        "[round_trip_with_tight_bound] Successfully read and processed {seen} total payload bytes within bounds"
    );
    Ok(())
}

fn read_over_limit_should_fail(bytes: &[u8]) {
    // A bound tighter than the frames on the stream: reading must fail with
    // InvalidFrame *before* any payload allocation happens.
    let deframer = DefaultDeframer::new().with_max_frame_len(4);
    let mut reader = StreamReader::new(Cursor::new(bytes), deframer);

    println!(
        "[read_over_limit_should_fail] Reading with a 4-byte bound against larger frames (expected error)"
    );
    let err = reader.process_all(|_| Ok(())).unwrap_err();
    match err.into_kind() {
        ErrorKind::InvalidFrame { .. } => {}
        other => panic!("expected InvalidFrame, got {other:?}"),
    }
}

fn main() -> Result<()> {
    // Happy path: write with manual BoundedFramer, read with a tightened bound
    println!(
        "[main] Starting bounded demonstration: enforcing maximum payload sizes during write and read"
    );
    let mut bytes = Vec::new();
    write_under_limit(&mut bytes)?;
    round_trip_with_tight_bound(&bytes)?;

    // Failure paths: write over the limit, read against a tighter bound
    let mut sink = Vec::new();
    write_over_limit_should_fail(&mut sink);
    read_over_limit_should_fail(&bytes);

    Ok(())
}
