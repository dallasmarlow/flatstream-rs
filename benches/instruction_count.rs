use flatbuffers::FlatBufferBuilder;
use flatstream::framing::{ChecksumDeframer, ChecksumFramer};
use flatstream::{
    DefaultDeframer, DefaultFramer, StreamReader, StreamSerialize, StreamWriter, XxHash64,
};
use iai_callgrind::{library_benchmark, library_benchmark_group, main};
use std::hint::black_box;
use std::io::Cursor;

// ---
// # Instruction-Count Suite (iai-callgrind)
//
// Callgrind instruction counts for four END-TO-END micro-workloads: default
// write/read and checksummed write/read over 100 telemetry frames. Unlike the
// wall-clock Criterion suite these counts are immune to scheduler/thermal
// noise — but they are NOT machine-universal constants: they are stable only
// for a pinned toolchain, dependency set, target, and flags. Compare runs
// within one pinned environment (scripts/instruction_counts.sh pins the
// container); treat deltas across environments as meaningless.
//
// Scope caveat: the measured functions include builder work and the output
// Vec's growth as well as framing — an instruction delta says "this workload
// changed," and isolating *which* loop changed is a follow-up (a framing-only
// group split is planned).
//
// Requires valgrind (Linux) and a matching `iai-callgrind-runner`; run via
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

#[library_benchmark]
#[bench::messages_100(setup = events)]
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

library_benchmark_group!(
    name = end_to_end_workloads;
    benchmarks = write_default, write_xxhash64, read_default, read_xxhash64
);

main!(library_benchmark_groups = end_to_end_workloads);
