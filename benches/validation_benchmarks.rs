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

/// Build a FlatBuffer containing a chain of nested empty tables.
/// Root table has one field pointing to the child table, recursively.
fn build_nested_empty_tables_bytes(depth: usize) -> Vec<u8> {
    assert!(depth >= 1, "depth must be >= 1");
    let mut b = FlatBufferBuilder::new();

    // Build from leaf to root
    let mut current: Option<flatbuffers::WIPOffset<flatbuffers::Table<'_>>> = None;
    for _ in 0..depth {
        let start = b.start_table();
        if let Some(child) = current {
            // Use first vtable slot (offset constant 4) to store the child table offset.
            b.push_slot_always::<flatbuffers::WIPOffset<_>>(4, child);
        }
        let this_table = b.end_table(start);
        // Convert finished table offset to a generic table offset type for nesting
        let as_table: flatbuffers::WIPOffset<flatbuffers::Table<'_>> =
            flatbuffers::WIPOffset::new(this_table.value());
        current = Some(as_table);
    }

    let root = current.expect("depth>=1 ensures a root table");
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

    let nested = build_nested_empty_tables_bytes(32);
    bench_write_group(
        "Validation: Write Path (nested empty tables, depth=32)",
        &nested,
        c,
    );
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

    let nested = build_framed(&build_nested_empty_tables_bytes(32));
    bench_read_group(
        "Validation: Read Path (nested empty tables, depth=32)",
        &nested,
        c,
    );

    // Strict limits variants (exercise VerifierOptions)
    let strict = TableRootValidator::with_limits(2, 1);
    let mut strict_read = c.benchmark_group("Validation: Read Path (telemetry, strict limits)");
    strict_read.bench_function("ValidatingDeframer + TableRootValidator (limits)", |b| {
        let d = DefaultDeframer.with_validator(strict);
        b.iter_batched(
            || (Cursor::new(telemetry.clone()), Vec::new()),
            |(mut r, mut buf)| {
                black_box(&d)
                    .read_and_deframe(&mut r, black_box(&mut buf))
                    .unwrap()
                    .unwrap();
            },
            BatchSize::SmallInput,
        )
    });
    strict_read.finish();
}

fn typed_validator() -> TypedValidator {
    TypedValidator::from_verify(|opts, payload| {
        telemetry_generated::telemetry::root_as_telemetry_event_with_opts(opts, payload).map(|_| ())
    })
}

fn bench_typed_validation_write_path(c: &mut Criterion) {
    let payload = build_telemetry_event_bytes();
    let mut group = c.benchmark_group("Typed Validation: Write Path (telemetry event)");

    group.bench_function("DefaultFramer (baseline)", |b| {
        b.iter_batched(
            sink,
            |mut w| {
                black_box(&DefaultFramer)
                    .frame_and_write(&mut w, black_box(&payload))
                    .unwrap();
            },
            BatchSize::SmallInput,
        )
    });

    group.bench_function("ValidatingFramer + TypedValidator", |b| {
        let framer = DefaultFramer.with_validator(typed_validator());
        b.iter_batched(
            sink,
            |mut w| {
                black_box(&framer)
                    .frame_and_write(&mut w, black_box(&payload))
                    .unwrap();
            },
            BatchSize::SmallInput,
        )
    });

    group.finish();
}

fn bench_typed_validation_read_path(c: &mut Criterion) {
    let framed = build_framed(&build_telemetry_event_bytes());
    let mut group = c.benchmark_group("Typed Validation: Read Path (telemetry event)");

    group.bench_function("DefaultDeframer (baseline)", |b| {
        b.iter_batched(
            || (Cursor::new(framed.clone()), Vec::new()),
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

    group.bench_function("ValidatingDeframer + TypedValidator", |b| {
        let d = DefaultDeframer.with_validator(typed_validator());
        b.iter_batched(
            || (Cursor::new(framed.clone()), Vec::new()),
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

criterion_group! {
    name = benches;
    config = Criterion::default().measurement_time(std::time::Duration::from_secs(10)).sample_size(100);
    targets = bench_validation_write_path, bench_validation_read_path, bench_typed_validation_write_path, bench_typed_validation_read_path
}
criterion_main!(benches);
