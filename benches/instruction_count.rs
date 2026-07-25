use flatbuffers::FlatBufferBuilder;
use flatstream::framing::{ChecksumDeframer, ChecksumFramer};
use flatstream::{
    Crc32, DefaultDeframer, DefaultFramer, Deframer, Durable, Framer, MemoryPolicy,
    ReclamationReason, StreamReader, StreamSerialize, StreamWriter, SyncEveryNFrames, SyncMode,
    XxHash64,
};
use gungraun::{library_benchmark, library_benchmark_group, main};
use std::hint::black_box;
use std::io::{self, Cursor, Write};
use std::num::NonZeroU64;

// ---
// # Instruction-Count Suite (Gungraun)
//
// Callgrind instruction counts for the framing micro-workloads over 100
// telemetry frames. Unlike the wall-clock Criterion suite these counts are
// immune to scheduler/thermal noise — but they are NOT machine-universal
// constants: they are stable only for a pinned toolchain, dependency set,
// target, and flags. Compare runs within one pinned environment
// (scripts/instruction_counts.sh pins the container); treat deltas across
// environments as meaningless.
//
// ## Position-accounting isolation (task A2)
//
// The end-to-end workloads below cannot, on their own, say how much of a
// write/read is the `StreamWriter`/`StreamReader` position accounting versus
// the builder and framing work they share. A2 isolates it by *pairing* each
// accounted path against a `*_direct` twin that performs byte-identical builder,
// framing, and checksum work but drives the `Framer`/`Deframer` straight over a
// raw `Cursor` — no `CountingWriter`/`CountingReader`, no receipt arithmetic,
// no `FrameReceipt`/`ReadFrame`. The instruction delta between a twin and its
// accounted sibling is therefore the accounting cost alone.
//
// Three writer arms and three reader arms per framing scheme make the
// distinction the task calls out — "counting wrappers and receipt arithmetic
// execute on every frame" — visible separately:
//
// - `*_direct`         — baseline: framing only, no accounting.
// - `write_accounted` / `write_crc32` / `read_*` — the common accounted path
//                        (`write_finished` / `process_all`). The byte counter
//                        increments every I/O call, but the receipt it feeds is
//                        discarded, so its arithmetic is a DCE candidate.
// - `*_receipt`        — `write_finished_with_receipt` /
//                        `process_all_with_receipt` with the receipt
//                        `black_box`ed so frame_start/wire_len math cannot be
//                        elided: the full receipt cost.
//
// Both schemes (default and CRC-32) are covered. The `write_xxhash64` /
// `read_xxhash64` and policy arms are retained unchanged as the continuity
// baseline for the E3/E4 findings recorded in the same environment.
//
// Requires valgrind (Linux) and a matching `gungraun-runner`; run via
// scripts/instruction_counts.sh. Gated behind the `instruction_bench` feature
// so plain `cargo bench` skips it. Stream/event construction for the read
// benches runs outside the measured section via setup functions.
// ---

const MESSAGE_COUNT: usize = 100;

struct TelemetryEvent {
    device_id: u64,
    timestamp: u64,
    value: f64,
}

impl StreamSerialize for TelemetryEvent {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> flatstream::Result<()> {
        // Stack-staged: a heap Vec here would put an allocation inside every
        // measured iteration and pollute the instruction counts.
        let mut data = [0u8; 24];
        data[..8].copy_from_slice(&self.device_id.to_le_bytes());
        data[8..16].copy_from_slice(&self.timestamp.to_le_bytes());
        data[16..].copy_from_slice(&self.value.to_le_bytes());

        let data_vec = builder.create_vector(&data);
        builder.finish(data_vec, None);
        Ok(())
    }
}

fn events() -> Vec<TelemetryEvent> {
    (0..MESSAGE_COUNT as u64)
        .map(|i| TelemetryEvent {
            device_id: i,
            timestamp: 1672531200 + i,
            value: i as f64 * 1.5,
        })
        .collect()
}

struct CountedDurable {
    inner: Cursor<Vec<u8>>,
    syncs: usize,
}

struct GateOpenNoOp;

impl MemoryPolicy for GateOpenNoOp {
    fn should_reset(&mut self, _: usize, _: usize) -> Option<ReclamationReason> {
        None
    }

    fn baseline_capacity(&self) -> usize {
        1
    }
}

impl CountedDurable {
    fn new() -> Self {
        Self {
            inner: Cursor::new(Vec::new()),
            syncs: 0,
        }
    }
}

impl Write for CountedDurable {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        self.inner.write_vectored(bufs)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Durable for CountedDurable {
    fn sync_data(&mut self) -> io::Result<()> {
        self.syncs += 1;
        Ok(())
    }

    fn sync_all(&mut self) -> io::Result<()> {
        self.syncs += 1;
        Ok(())
    }
}

fn default_stream() -> Vec<u8> {
    let mut buf = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buf), DefaultFramer);
    for event in events() {
        writer.write(&event).unwrap();
    }
    buf
}

fn xxhash64_stream() -> Vec<u8> {
    let mut buf = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buf), ChecksumFramer::new(XxHash64::new()));
    for event in events() {
        writer.write(&event).unwrap();
    }
    buf
}

fn crc32_stream() -> Vec<u8> {
    let mut buf = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buf), ChecksumFramer::new(Crc32::new()));
    for event in events() {
        writer.write(&event).unwrap();
    }
    buf
}

fn events_with_writer_parity() -> Vec<TelemetryEvent> {
    let events = events();
    assert_eq!(
        frame_all_direct_bytes(&events, &DefaultFramer),
        frame_all_accounted_bytes(&events, DefaultFramer),
        "default direct/accounted baselines must emit identical bytes"
    );
    assert_eq!(
        frame_all_direct_bytes(&events, &ChecksumFramer::new(Crc32::new())),
        frame_all_accounted_bytes(&events, ChecksumFramer::new(Crc32::new())),
        "CRC-32 direct/accounted baselines must emit identical bytes"
    );
    events
}

#[library_benchmark]
#[bench::messages_100(setup = events_with_writer_parity)]
fn write_default(events: Vec<TelemetryEvent>) -> usize {
    let mut buf = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buf), DefaultFramer);
    for event in &events {
        writer.write(black_box(event)).unwrap();
    }
    buf.len()
}

#[library_benchmark]
#[bench::messages_100(setup = events)]
fn write_xxhash64(events: Vec<TelemetryEvent>) -> usize {
    let mut buf = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buf), ChecksumFramer::new(XxHash64::new()));
    for event in &events {
        writer.write(black_box(event)).unwrap();
    }
    buf.len()
}

// --- A2 writer position-accounting isolation ---
//
// `write_direct` and `write_crc32_direct` are the accounting-free baselines:
// they reuse one `FlatBufferBuilder` and call the framer straight over a raw
// `Cursor`, doing byte-identical serialize + frame work to the accounted arms
// but without the `CountingWriter`, receipt arithmetic, or sync/memory backend
// calls a `StreamWriter` runs on every frame.

fn frame_all_direct_bytes<F: Framer>(events: &[TelemetryEvent], framer: &F) -> Vec<u8> {
    let mut builder = FlatBufferBuilder::new();
    let mut buf = Vec::new();
    // Match the accounted arms' sink exactly (`Cursor<&mut Vec<u8>>`) so the
    // only difference measured is the accounting the `StreamWriter` layers on.
    let mut sink = Cursor::new(&mut buf);
    for event in events {
        builder.reset();
        black_box(event).serialize(&mut builder).unwrap();
        let payload = builder.finished_data();
        framer.frame_and_write(&mut sink, payload).unwrap();
    }
    buf
}

fn frame_all_direct<F: Framer>(events: &[TelemetryEvent], framer: &F) -> usize {
    frame_all_direct_bytes(events, framer).len()
}

fn frame_all_accounted_bytes<F: Framer>(events: &[TelemetryEvent], framer: F) -> Vec<u8> {
    let mut builder = FlatBufferBuilder::new();
    let mut buf = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buf), framer);
    for event in events {
        builder.reset();
        black_box(event).serialize(&mut builder).unwrap();
        writer.write_finished(&mut builder).unwrap();
    }
    drop(writer);
    buf
}

#[library_benchmark]
#[bench::messages_100(setup = events_with_writer_parity)]
fn write_direct(events: Vec<TelemetryEvent>) -> usize {
    frame_all_direct(&events, &DefaultFramer)
}

#[library_benchmark]
#[bench::messages_100(setup = events_with_writer_parity)]
fn write_accounted(events: Vec<TelemetryEvent>) -> usize {
    let mut builder = FlatBufferBuilder::new();
    let mut buf = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buf), DefaultFramer);
    for event in &events {
        builder.reset();
        black_box(event).serialize(&mut builder).unwrap();
        writer.write_finished(&mut builder).unwrap();
    }
    buf.len()
}

#[library_benchmark]
#[bench::messages_100(setup = events_with_writer_parity)]
fn write_receipt(events: Vec<TelemetryEvent>) -> usize {
    let mut builder = FlatBufferBuilder::new();
    let mut buf = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buf), DefaultFramer);
    let mut receipt_acc = 0u64;
    for event in &events {
        builder.reset();
        black_box(event).serialize(&mut builder).unwrap();
        let receipt = writer.write_finished_with_receipt(&mut builder).unwrap();
        receipt_acc = receipt_acc
            .wrapping_add(receipt.frame_start)
            .wrapping_add(receipt.wire_len);
    }
    black_box((buf.len(), receipt_acc)).0
}

#[library_benchmark]
#[bench::messages_100(setup = events_with_writer_parity)]
fn write_crc32_direct(events: Vec<TelemetryEvent>) -> usize {
    let mut builder = FlatBufferBuilder::new();
    let mut buf = Vec::new();
    let mut sink = Cursor::new(&mut buf);
    let framer = ChecksumFramer::new(Crc32::new());
    for event in &events {
        builder.reset();
        black_box(event).serialize(&mut builder).unwrap();
        framer
            .frame_and_write(&mut sink, builder.finished_data())
            .unwrap();
    }
    buf.len()
}

#[library_benchmark]
#[bench::messages_100(setup = events_with_writer_parity)]
fn write_crc32(events: Vec<TelemetryEvent>) -> usize {
    let mut builder = FlatBufferBuilder::new();
    let mut buf = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buf), ChecksumFramer::new(Crc32::new()));
    for event in &events {
        builder.reset();
        black_box(event).serialize(&mut builder).unwrap();
        writer.write_finished(&mut builder).unwrap();
    }
    buf.len()
}

#[library_benchmark]
#[bench::messages_100(setup = events_with_writer_parity)]
fn write_crc32_receipt(events: Vec<TelemetryEvent>) -> usize {
    let mut builder = FlatBufferBuilder::new();
    let mut buf = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buf), ChecksumFramer::new(Crc32::new()));
    let mut receipt_acc = 0u64;
    for event in &events {
        builder.reset();
        black_box(event).serialize(&mut builder).unwrap();
        let receipt = writer.write_finished_with_receipt(&mut builder).unwrap();
        receipt_acc = receipt_acc
            .wrapping_add(receipt.frame_start)
            .wrapping_add(receipt.wire_len);
    }
    black_box((buf.len(), receipt_acc)).0
}

#[library_benchmark]
#[bench::messages_100(setup = events)]
fn write_static_sync_policy_not_due(events: Vec<TelemetryEvent>) -> usize {
    let policy = SyncEveryNFrames::new(
        NonZeroU64::new((MESSAGE_COUNT + 1) as u64).unwrap(),
        SyncMode::Data,
    );
    let mut writer =
        StreamWriter::new(CountedDurable::new(), DefaultFramer).with_sync_policy(policy);
    for event in &events {
        writer.write(black_box(event)).unwrap();
    }
    writer.into_inner().inner.into_inner().len()
}

#[library_benchmark]
#[bench::messages_100(setup = events)]
fn write_static_sync_policy_once(events: Vec<TelemetryEvent>) -> usize {
    let policy = SyncEveryNFrames::new(
        NonZeroU64::new(MESSAGE_COUNT as u64).unwrap(),
        SyncMode::Data,
    );
    let mut writer =
        StreamWriter::new(CountedDurable::new(), DefaultFramer).with_sync_policy(policy);
    for event in &events {
        writer.write(black_box(event)).unwrap();
    }
    let sink = writer.into_inner();
    black_box(sink.syncs);
    sink.inner.into_inner().len()
}

#[library_benchmark]
#[bench::messages_100(setup = events)]
fn write_static_memory_policy_not_due(events: Vec<TelemetryEvent>) -> usize {
    let mut writer =
        StreamWriter::new(Cursor::new(Vec::new()), DefaultFramer).with_memory_policy(GateOpenNoOp);
    for event in &events {
        writer.write(black_box(event)).unwrap();
    }
    writer.into_inner().into_inner().len()
}

#[library_benchmark]
#[bench::messages_100(setup = default_stream)]
fn read_default(data: Vec<u8>) -> usize {
    let mut reader = StreamReader::new(Cursor::new(data), DefaultDeframer::new());
    let mut total = 0;
    reader
        .process_all(|payload| {
            total += black_box(payload).len();
            Ok(())
        })
        .unwrap();
    total
}

#[library_benchmark]
#[bench::messages_100(setup = xxhash64_stream)]
fn read_xxhash64(data: Vec<u8>) -> usize {
    let mut reader = StreamReader::new(Cursor::new(data), ChecksumDeframer::new(XxHash64::new()));
    let mut total = 0;
    reader
        .process_all(|payload| {
            total += black_box(payload).len();
            Ok(())
        })
        .unwrap();
    total
}

// --- A2 reader position-accounting isolation ---
//
// `read_*_direct` drive the `Deframer` straight over a raw `Cursor` with a
// caller-owned buffer, doing byte-identical header/checksum/payload work to the
// accounted arms but without the `CountingReader` byte tally, `bytes_consumed`
// arithmetic, or `ReadFrame`/`FrameReceipt` construction a `StreamReader` runs
// per frame. Every arm calls the same non-inlined payload consumer so LLVM
// cannot replace the read with a known length.

#[inline(never)]
fn consume_payload(payload: &[u8]) -> usize {
    payload
        .iter()
        .fold(0usize, |acc, byte| acc.rotate_left(5) ^ usize::from(*byte))
}

fn deframe_all_direct<D: Deframer>(data: &[u8], deframer: &D) -> usize {
    let mut reader = Cursor::new(data);
    let mut buffer = Vec::new();
    let mut total = 0usize;
    while let Some(n) = deframer.read_and_deframe(&mut reader, &mut buffer).unwrap() {
        total = total.wrapping_add(consume_payload(&buffer[..n]));
    }
    total
}

#[library_benchmark]
#[bench::messages_100(setup = default_stream)]
fn read_direct(data: Vec<u8>) -> usize {
    deframe_all_direct(&data, &DefaultDeframer::new())
}

#[library_benchmark]
#[bench::messages_100(setup = default_stream)]
fn read_accounted(data: Vec<u8>) -> usize {
    let mut reader = StreamReader::new(Cursor::new(data), DefaultDeframer::new());
    let mut total = 0usize;
    reader
        .process_all(|payload| {
            total = total.wrapping_add(consume_payload(payload));
            Ok(())
        })
        .unwrap();
    total
}

#[library_benchmark]
#[bench::messages_100(setup = default_stream)]
fn read_receipt(data: Vec<u8>) -> usize {
    let mut reader = StreamReader::new(Cursor::new(data), DefaultDeframer::new());
    let mut total = 0usize;
    let mut receipt_acc = 0u64;
    reader
        .process_all_with_receipt(|frame| {
            total = total.wrapping_add(consume_payload(frame.payload));
            receipt_acc = receipt_acc
                .wrapping_add(frame.receipt.frame_start)
                .wrapping_add(frame.receipt.wire_len);
            Ok(())
        })
        .unwrap();
    black_box((total, receipt_acc)).0
}

#[library_benchmark]
#[bench::messages_100(setup = crc32_stream)]
fn read_crc32_direct(data: Vec<u8>) -> usize {
    deframe_all_direct(&data, &ChecksumDeframer::new(Crc32::new()))
}

#[library_benchmark]
#[bench::messages_100(setup = crc32_stream)]
fn read_crc32(data: Vec<u8>) -> usize {
    let mut reader = StreamReader::new(Cursor::new(data), ChecksumDeframer::new(Crc32::new()));
    let mut total = 0usize;
    reader
        .process_all(|payload| {
            total = total.wrapping_add(consume_payload(payload));
            Ok(())
        })
        .unwrap();
    total
}

#[library_benchmark]
#[bench::messages_100(setup = crc32_stream)]
fn read_crc32_receipt(data: Vec<u8>) -> usize {
    let mut reader = StreamReader::new(Cursor::new(data), ChecksumDeframer::new(Crc32::new()));
    let mut total = 0usize;
    let mut receipt_acc = 0u64;
    reader
        .process_all_with_receipt(|frame| {
            total = total.wrapping_add(consume_payload(frame.payload));
            receipt_acc = receipt_acc
                .wrapping_add(frame.receipt.frame_start)
                .wrapping_add(frame.receipt.wire_len);
            Ok(())
        })
        .unwrap();
    black_box((total, receipt_acc)).0
}

library_benchmark_group!(
    name = end_to_end_workloads;
    benchmarks =
        write_default,
        write_xxhash64,
        write_static_memory_policy_not_due,
        write_static_sync_policy_not_due,
        write_static_sync_policy_once,
        read_default,
        read_xxhash64,
        // A2 writer position-accounting isolation (default + CRC-32)
        write_direct,
        write_accounted,
        write_receipt,
        write_crc32_direct,
        write_crc32,
        write_crc32_receipt,
        // A2 reader position-accounting isolation (default + CRC-32)
        read_direct,
        read_accounted,
        read_receipt,
        read_crc32_direct,
        read_crc32,
        read_crc32_receipt
);

main!(library_benchmark_groups = end_to_end_workloads);
