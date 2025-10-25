use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use flatbuffers::FlatBufferBuilder;
use flatstream::framing::{DeframerExt, FramerExt};
use flatstream::*;
use std::io::{sink, Cursor};

fn build_empty_table_bytes() -> Vec<u8> {
    let mut b = FlatBufferBuilder::new();
    let start = b.start_table();
    let root = b.end_table(start);
    b.finish(root, None);
    b.finished_data().to_vec()
}

fn build_framed(buf: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    DefaultFramer.frame_and_write(&mut out, buf).unwrap();
    out
}

// Zero-cost check: Adding NoValidator should be indistinguishable from baseline
fn bench_validation_write_path(c: &mut Criterion) {
    let payload = build_empty_table_bytes();
    let mut group = c.benchmark_group("Validation: Write Path");

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

    group.bench_function("ValidatingFramer + NoValidator (zero-cost)", |b| {
        let framer = DefaultFramer.with_validator(NoValidator);
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

    group.bench_function("ValidatingFramer + StructuralValidator", |b| {
        let framer = DefaultFramer.with_validator(StructuralValidator::new());
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

fn bench_validation_read_path(c: &mut Criterion) {
    let payload = build_empty_table_bytes();
    let framed = build_framed(&payload);
    let mut group = c.benchmark_group("Validation: Read Path");

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

    group.bench_function("ValidatingDeframer + NoValidator (zero-cost)", |b| {
        let d = DefaultDeframer.with_validator(NoValidator);
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

    group.bench_function("ValidatingDeframer + StructuralValidator", |b| {
        let d = DefaultDeframer.with_validator(StructuralValidator::new());
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

criterion_group!(
    benches,
    bench_validation_write_path,
    bench_validation_read_path
);
criterion_main!(benches);
