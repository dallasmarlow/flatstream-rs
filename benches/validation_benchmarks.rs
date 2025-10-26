use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use flatbuffers::FlatBufferBuilder;
use flatstream::framing::{DeframerExt, FramerExt};
use flatstream::*;
use std::io::{sink, Cursor};

// Use generated telemetry to exercise a non-empty table
#[allow(clippy::extra_unused_lifetimes, mismatched_lifetime_syntaxes)]
#[path = "../examples/generated/telemetry_generated.rs"]
mod telemetry_generated;

fn build_empty_table_bytes() -> Vec<u8> {
    let mut b = FlatBufferBuilder::new();
    let start = b.start_table();
    let root = b.end_table(start);
    b.finish(root, None);
    b.finished_data().to_vec()
}

fn build_telemetry_event_bytes() -> Vec<u8> {
    let mut b = FlatBufferBuilder::new();
    let msg = b.create_string("hello");
    let mut tb = telemetry_generated::telemetry::TelemetryEventBuilder::new(&mut b);
    tb.add_message(msg);
    tb.add_timestamp(123);
    let root = tb.finish();
    b.finish(root, None);
    b.finished_data().to_vec()
}

fn build_framed(buf: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    DefaultFramer.frame_and_write(&mut out, buf).unwrap();
    out
}

fn bench_write_group(group_name: &str, payload: &[u8], c: &mut Criterion) {
    let mut group = c.benchmark_group(group_name);

    group.bench_function("DefaultFramer (baseline)", |b| {
        b.iter_batched(
            sink,
            |mut w| {
                black_box(&DefaultFramer)
                    .frame_and_write(&mut w, black_box(payload))
                    .unwrap();
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("ValidatingFramer + NoValidator", |b| {
        let framer = DefaultFramer.with_validator(NoValidator);
        b.iter_batched(
            sink,
            |mut w| {
                black_box(&framer)
                    .frame_and_write(&mut w, black_box(payload))
                    .unwrap();
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("ValidatingFramer + TableRootValidator", |b| {
        let framer = DefaultFramer.with_validator(TableRootValidator::new());
        b.iter_batched(
            sink,
            |mut w| {
                black_box(&framer)
                    .frame_and_write(&mut w, black_box(payload))
                    .unwrap();
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

// Zero-cost check + realistic payloads
fn bench_validation_write_path(c: &mut Criterion) {
    let empty = build_empty_table_bytes();
    bench_write_group("Validation: Write Path (empty table)", &empty, c);

    let telemetry = build_telemetry_event_bytes();
    bench_write_group("Validation: Write Path (telemetry event)", &telemetry, c);
}

fn bench_read_group(group_name: &str, framed: &[u8], c: &mut Criterion) {
    let mut group = c.benchmark_group(group_name);

    group.bench_function("DefaultDeframer (baseline)", |b| {
        b.iter_batched(
            || (Cursor::new(framed.to_vec()), Vec::new()),
            |(mut r, mut buf)| {
                let d = DefaultDeframer;
                black_box(&d)
                    .read_and_deframe(&mut r, black_box(&mut buf))
                    .unwrap()
                    .unwrap();
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("ValidatingDeframer + NoValidator", |b| {
        let d = DefaultDeframer.with_validator(NoValidator);
        b.iter_batched(
            || (Cursor::new(framed.to_vec()), Vec::new()),
            |(mut r, mut buf)| {
                black_box(&d)
                    .read_and_deframe(&mut r, black_box(&mut buf))
                    .unwrap()
                    .unwrap();
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("ValidatingDeframer + TableRootValidator", |b| {
        let d = DefaultDeframer.with_validator(TableRootValidator::new());
        b.iter_batched(
            || (Cursor::new(framed.to_vec()), Vec::new()),
            |(mut r, mut buf)| {
                black_box(&d)
                    .read_and_deframe(&mut r, black_box(&mut buf))
                    .unwrap()
                    .unwrap();
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

fn bench_validation_read_path(c: &mut Criterion) {
    let empty = build_framed(&build_empty_table_bytes());
    bench_read_group("Validation: Read Path (empty table)", &empty, c);

    let telemetry = build_framed(&build_telemetry_event_bytes());
    bench_read_group("Validation: Read Path (telemetry event)", &telemetry, c);
}

criterion_group! {
    name = benches;
    config = Criterion::default().measurement_time(std::time::Duration::from_secs(10)).sample_size(100);
    targets = bench_validation_write_path, bench_validation_read_path
}
criterion_main!(benches);
