// Example purpose: B3 observability recipe. Shows the *operation boundary* —
// not the payload-inspection point — as the correct place to observe frame
// writes, reads, and their success/failure. `ObserverFramer` fires its callback
// before I/O and so cannot report success, receipt bounds, or latency; the
// receipt-returning writer/reader APIs can. See
// `docs/planning/B3_OBSERVABILITY_BOUNDARY.md`.
//
//! A dependency-free observability recipe built entirely on flatstream's public
//! receipt APIs. No OTEL, no metrics crate, nothing added to flatstream's hot
//! path: the application wraps its own `write` / `read` call site, where both
//! the `Result` (success vs. failure) and the `FrameReceipt` (frame bounds) are
//! in hand and a timer brackets the real I/O.
//!
//! The key property asserted here is the one `ObserverFramer` cannot provide: a
//! write that fails increments a *failure* counter and leaves the success
//! counter and the recorded-byte total untouched — errors are never counted as
//! successful frames. The durability leg pins the second half of the contract:
//! a failed checkpoint classifies as a durability failure whose bytes *were*
//! accepted by the sink, which is exactly why the caller must not re-emit them.

use flatstream::{
    DefaultDeframer, DefaultFramer, Durable, ErrorKind, Result, StreamReader, StreamWriter,
    SyncEveryFrame, SyncMode,
};
use std::io::{Cursor, Write};

/// Plain in-process telemetry. In a real application these fields map to your
/// metrics backend (OTEL counters, Prometheus gauges, a log line); flatstream
/// deliberately knows nothing about that mapping.
#[derive(Default, Debug)]
struct WriteTelemetry {
    frames_ok: u64,
    frames_failed: u64,
    /// Sum of `wire_len` over successful frames only.
    bytes_ok: u64,
    /// Contiguous ranges of the successful frames, in write order.
    ranges: Vec<std::ops::Range<u64>>,
    /// A separate tally for the durability-failure case, whose bytes are
    /// already on the wire even though stable storage was not confirmed.
    durability_failures: u64,
    /// Stream position the last failed checkpoint could not confirm. Bytes up
    /// to here were accepted by the sink but are not known durable — the caller
    /// must neither count them as ok nor re-emit them.
    unsynced_watermark: Option<u64>,
}

impl WriteTelemetry {
    /// Records the outcome of one `write_with_receipt` call. This is the whole
    /// recipe: branch on the `Result` so a failure can never land on the
    /// success counters.
    fn record(&mut self, outcome: &Result<flatstream::FrameReceipt>) {
        match outcome {
            Ok(receipt) => {
                self.frames_ok += 1;
                self.bytes_ok += receipt.wire_len;
                self.ranges.push(receipt.range());
            }
            Err(e) => {
                self.frames_failed += 1;
                // Distinguish "bytes never fully written" from "bytes written
                // but not made durable" — they demand different recovery.
                if let ErrorKind::DurabilityFailed {
                    attempted_watermark,
                    ..
                } = e.kind()
                {
                    self.durability_failures += 1;
                    self.unsynced_watermark = Some(*attempted_watermark);
                }
            }
        }
    }
}

/// Accepts `fail_after` bytes, then fails every subsequent write. Lets us prove
/// the failure path without any real I/O or platform dependency.
struct FailingSink {
    written: usize,
    fail_after: usize,
}

impl Write for FailingSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.written >= self.fail_after {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "sink refused further bytes",
            ));
        }
        let n = (self.fail_after - self.written).min(buf.len());
        self.written += n;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Accepts every byte but can never confirm a durability checkpoint — the shape
/// of a device that takes writes and then fails at `fdatasync`. It only ever
/// *fails* to sync, so it does not violate the `Durable` contract's rule
/// against in-memory sinks reporting durability they don't have.
#[derive(Default)]
struct UnsyncableSink {
    accepted: Vec<u8>,
}

impl Write for UnsyncableSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.accepted.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Durable for UnsyncableSink {
    fn sync_data(&mut self) -> std::io::Result<()> {
        Err(std::io::Error::other("device cannot confirm durability"))
    }

    fn sync_all(&mut self) -> std::io::Result<()> {
        self.sync_data()
    }
}

fn main() -> Result<()> {
    happy_path_write_then_read()?;
    failure_is_never_counted_as_success()?;
    durability_failure_is_classified_and_bytes_were_accepted()?;
    println!("observability_boundary: all boundary-observability assertions held");
    Ok(())
}

/// Property 1 + 2 + 4: a clean multi-frame write records exactly the frames
/// written, the recorded byte total equals `bytes_written()`, the recorded
/// ranges tile the stream contiguously, and the read side recovers the same
/// count and the same ranges.
fn happy_path_write_then_read() -> Result<()> {
    let messages = ["alpha", "bravo", "charlie"];

    let mut buf = Vec::new();
    let mut tel = WriteTelemetry::default();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut buf), DefaultFramer);
        for m in messages {
            // The recipe: observe at the boundary, timing the real I/O call.
            // (A production wrapper would sample a clock around this line; we
            // omit the clock so the example stays deterministic.)
            let outcome = writer.write_with_receipt(&m);
            tel.record(&outcome);
            outcome?; // propagate any real error after recording it
        }
        writer.flush()?;
        // The recorded byte total must equal the writer's own accounting.
        assert_eq!(tel.bytes_ok, writer.bytes_written());
    }

    // Property 1: every frame recorded once, no failures.
    assert_eq!(tel.frames_ok, messages.len() as u64);
    assert_eq!(tel.frames_failed, 0);
    assert_eq!(tel.durability_failures, 0);

    // Property 2: recorded ranges tile the stream contiguously from offset 0
    // and cover exactly the bytes on the wire.
    assert_eq!(tel.ranges.first().unwrap().start, 0);
    for pair in tel.ranges.windows(2) {
        assert_eq!(pair[0].end, pair[1].start, "frames must not overlap or gap");
    }
    assert_eq!(tel.ranges.last().unwrap().end, buf.len() as u64);

    // Property 4: read the same stream back through the receipt-aware boundary
    // and recover the identical frame count and ranges.
    let mut read_ranges = Vec::new();
    let mut read_count = 0u64;
    let mut reader = StreamReader::new(Cursor::new(&buf), DefaultDeframer::new());
    reader.process_all_with_receipt(|frame| {
        read_count += 1;
        read_ranges.push(frame.receipt.range());
        Ok(())
    })?;
    assert_eq!(read_count, tel.frames_ok);
    assert_eq!(read_ranges, tel.ranges);

    println!(
        "[write+read] {} frames, {} bytes, ranges tile [0, {}) contiguously on both sides",
        tel.frames_ok,
        tel.bytes_ok,
        buf.len()
    );
    Ok(())
}

/// Property 3 — the reason B3 exists: a failing write increments the failure
/// counter and touches neither the success counter nor the recorded-byte total.
/// This is exactly what an `ObserverFramer` callback (which fires *before* I/O)
/// cannot guarantee.
fn failure_is_never_counted_as_success() -> Result<()> {
    // Accept the first frame, then refuse — the second write fails mid-frame.
    // Measure the first frame's exact wire length with a trial write, so the
    // sink's byte limit admits exactly one frame regardless of how FlatBuffers
    // sizes the payload.
    let first = "ok-frame";
    let first_frame_len = {
        let mut probe = StreamWriter::new(Cursor::new(Vec::new()), DefaultFramer);
        probe.write_with_receipt(&first)?.wire_len as usize
    };
    let sink = FailingSink {
        written: 0,
        fail_after: first_frame_len,
    };

    let mut tel = WriteTelemetry::default();
    let mut writer = StreamWriter::new(sink, DefaultFramer);

    let first_outcome = writer.write_with_receipt(&first);
    tel.record(&first_outcome);
    first_outcome.expect("first frame fits under the sink's byte limit");

    let bytes_after_first = tel.bytes_ok;
    let ok_after_first = tel.frames_ok;

    // Second write must fail at the sink; record it and confirm classification.
    let second_outcome = writer.write_with_receipt(&"this-frame-cannot-be-written");
    tel.record(&second_outcome);
    match second_outcome {
        Err(e) => assert!(
            matches!(e.kind(), ErrorKind::Io(_)),
            "expected an Io failure from the refusing sink, got {:?}",
            e.kind()
        ),
        Ok(r) => panic!("write unexpectedly succeeded: {r:?}"),
    }

    // The failure landed on the failure counter and nowhere else.
    assert_eq!(tel.frames_failed, 1);
    assert_eq!(tel.frames_ok, ok_after_first, "success count must not move");
    assert_eq!(
        tel.bytes_ok, bytes_after_first,
        "recorded bytes must not move on failure"
    );
    // This particular failure is not a durability failure — the bytes were
    // refused outright, not accepted-then-unsynced.
    assert_eq!(tel.durability_failures, 0);

    println!(
        "[failure] 1 frame ok ({} bytes), 1 write refused and counted as a failure, \
         not a success",
        tel.bytes_ok
    );
    Ok(())
}

/// Property 5 — the durability half of the recipe: with an automatic sync
/// policy installed, a failed checkpoint surfaces as `Err(DurabilityFailed)`
/// from the write call itself. The telemetry must classify it as a durability
/// failure — distinct from an I/O refusal, because its bytes are already on the
/// wire and must not be re-emitted — while the success counters stay untouched.
fn durability_failure_is_classified_and_bytes_were_accepted() -> Result<()> {
    let mut tel = WriteTelemetry::default();
    let mut writer = StreamWriter::new(UnsyncableSink::default(), DefaultFramer)
        .with_sync_policy(SyncEveryFrame::new(SyncMode::Data));

    let outcome = writer.write_with_receipt(&"accepted-but-not-durable");
    tel.record(&outcome);

    let err = match outcome {
        Err(e) => e,
        Ok(r) => panic!("checkpoint against an unsyncable sink must fail: {r:?}"),
    };
    match err.kind() {
        ErrorKind::DurabilityFailed {
            attempted_watermark,
            previous_watermark,
            frame_start,
            wire_len,
            ..
        } => {
            // "Durability failure occurs after bytes were accepted": the frame
            // is on the wire even though the write call returned Err. The
            // watermark the checkpoint attempted equals both the writer's
            // advanced position and the bytes the sink actually holds.
            assert_eq!(*attempted_watermark, writer.bytes_written());
            assert_eq!(*previous_watermark, None, "no checkpoint ever succeeded");
            assert_eq!(*frame_start, Some(0), "triggering frame starts the stream");
            assert_eq!(
                Some(*attempted_watermark),
                *wire_len,
                "single triggering frame spans the whole stream"
            );
        }
        other => panic!("expected DurabilityFailed, got {other:?}"),
    }
    let accepted = writer.into_inner().accepted;
    assert_eq!(tel.unsynced_watermark, Some(accepted.len() as u64));

    // Classification: one durability failure, no successes, nothing durable.
    assert_eq!(tel.frames_failed, 1);
    assert_eq!(tel.durability_failures, 1);
    assert_eq!(tel.frames_ok, 0);
    assert_eq!(tel.bytes_ok, 0, "unsynced bytes are never counted as ok");

    println!(
        "[durability] 1 checkpoint failed after the sink accepted {} bytes; \
         classified as a durability failure, not a success and not an I/O refusal",
        accepted.len()
    );
    Ok(())
}
