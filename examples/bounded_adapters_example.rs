// Example purpose: Shows how to enforce frame/payload limits — the bounded framer
// adapter on the write path, the deframers' built-in `max_frame_len` on the read
// path — and what errors to expect when limits are exceeded (InvalidFrame with context).
//! Demonstrates enforcing maximum payload sizes on both write and read paths.
//! Reading defaults to the FlatBuffers maximum buffer size (2 GiB);
//! `with_max_frame_len` tightens the bound for untrusted input.

use flatstream::framing::{BoundedFramer, FramerExt};
use flatstream::{DefaultDeframer, DefaultFramer, ErrorKind, Result, StreamReader, StreamWriter};
use std::io::Cursor;

fn write_under_limit(bytes: &mut Vec<u8>) -> Result<()> {
    // Enforce a generous max payload length to accommodate FlatBuffer overhead
    let framer = BoundedFramer::new(DefaultFramer, 64);
    let writer = Cursor::new(&mut *bytes);
    let mut stream_writer = StreamWriter::new(writer, framer);

    // Simple mode: `&str` implements StreamSerialize in this crate
    println!(
        "[write_under_limit] Writing a small message within the configured 64-byte bound using a bounded framer"
    );
    stream_writer.write(&"hello")?; // 5 bytes, ok
    stream_writer.flush()?;
    drop(stream_writer);

    assert!(
        !bytes.is_empty(),
        "an accepted write must actually reach the sink"
    );
    let declared = u32::from_le_bytes(bytes[..4].try_into().expect("length prefix")) as usize;
    assert_eq!(
        bytes.len(),
        4 + declared,
        "the bounded framer must emit a well-formed [len][payload] frame"
    );
    assert!(
        declared <= 64,
        "payload of {declared} bytes slipped past the 64-byte bound"
    );
    Ok(())
}

fn write_over_limit_should_fail(bytes: &mut Vec<u8>) {
    // Use fluent composition for the framer
    let framer = DefaultFramer.bounded(4);
    let writer = Cursor::new(&mut *bytes);
    let mut stream_writer = StreamWriter::new(writer, framer);

    println!(
        "[write_over_limit_should_fail] Attempting to write a message that exceeds the configured 4-byte bound (expected error)"
    );
    let err = stream_writer.write(&"hello").unwrap_err(); // 5 bytes exceeds 4
    match err.into_kind() {
        ErrorKind::InvalidFrame { .. } => {}
        other => panic!("expected InvalidFrame, got {other:?}"),
    }
    drop(stream_writer);

    // The bound is a gate, not a truncation: rejecting must leave the stream
    // untouched, since a half-written frame would corrupt everything after it.
    assert!(
        bytes.is_empty(),
        "a rejected write leaked {} bytes into the stream",
        bytes.len()
    );
}

fn round_trip_with_tight_bound(bytes: &[u8]) -> Result<()> {
    // Same generous limit configured directly on the deframer
    let deframer = DefaultDeframer::new().with_max_frame_len(64);
    let mut reader = StreamReader::new(Cursor::new(bytes), deframer);

    let mut seen = 0usize;
    let mut messages = 0usize;
    println!(
        "[round_trip_with_tight_bound] Reading all messages with the deframer's max_frame_len tightened to 64 bytes"
    );
    reader.process_all(|payload| {
        // `payload` is a borrowed slice: zero-copy
        seen += payload.len();
        messages += 1;
        // The bound must admit the message *and* leave it intact.
        assert_eq!(
            flatbuffers::root::<&str>(payload).expect("payload is a valid FlatBuffers string"),
            "hello",
            "a bound that admits a frame must not alter it"
        );
        Ok(())
    })?;

    // Without this, a deframer that silently yielded nothing would still
    // "succeed" here — the failure mode this example exists to rule out.
    assert_eq!(messages, 1, "expected exactly one message on the stream");
    assert!(seen > 0, "message read but payload was empty");

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
    let mut delivered = 0usize;
    let err = reader
        .process_all(|_| {
            delivered += 1;
            Ok(())
        })
        .unwrap_err();
    match err.into_kind() {
        ErrorKind::InvalidFrame { .. } => {}
        other => panic!("expected InvalidFrame, got {other:?}"),
    }

    // The bound exists to stop a hostile length prefix from driving an
    // allocation, so rejection must happen before the payload is ever read —
    // never after handing it to the callback.
    assert_eq!(
        delivered, 0,
        "an over-limit frame was delivered to the callback before being rejected"
    );
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
