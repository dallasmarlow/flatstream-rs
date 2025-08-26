use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::io::sink;

// A tiny message used for the practical write-path benchmark
struct TestMsg;

impl StreamSerialize for TestMsg {
    #[inline(never)]
    fn serialize<A: flatbuffers::Allocator>(&self, b: &mut FlatBufferBuilder<A>) -> Result<()> {
        let s = b.create_string("");
        b.finish(s, None);
        Ok(())
    }
}

// A no-op message used to isolate the pure call overhead
struct NoopMsg;

impl StreamSerialize for NoopMsg {
    #[inline(never)]
    fn serialize<A: flatbuffers::Allocator>(&self, _b: &mut FlatBufferBuilder<A>) -> Result<()> {
        Ok(())
    }
}

fn bench_practical_write_path(c: &mut Criterion) {
    let mut group = c.benchmark_group("Practical Write-Path: Simple vs. Expert Mode");

    group.bench_function("Simple Mode (with call)", |b| {
        b.iter_with_setup(
            || {
                let writer = StreamWriter::new(sink(), DefaultFramer);
                (writer, TestMsg)
            },
            |(mut writer, msg)| {
                black_box(&mut writer).write(black_box(&msg)).unwrap();
            },
        )
    });

    group.bench_function("Expert Mode (no call in loop)", |b| {
        b.iter_with_setup(
            || {
                let writer = StreamWriter::new(sink(), DefaultFramer);
                let mut builder = FlatBufferBuilder::new();
                // Pre-serialize once, so the loop only measures the write path
                TestMsg.serialize(&mut builder).unwrap();
                (writer, builder)
            },
            |(mut writer, mut builder)| {
                black_box(&mut writer)
                    .write_finished(black_box(&mut builder))
                    .unwrap();
            },
        )
    });

    group.finish();
}

fn bench_pure_call_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("Pure Call Overhead: StreamSerialize Dispatch");

    group.bench_function("Static call: Noop serialize()", |b| {
        b.iter_with_setup(FlatBufferBuilder::new, |mut builder| {
            NoopMsg.serialize(black_box(&mut builder)).unwrap();
        })
    });

    group.bench_function("Baseline: No call", |b| {
        b.iter_with_setup(FlatBufferBuilder::new, |mut builder| {
            // Touch the builder so the optimizer can't elide it entirely
            black_box(&mut builder);
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_practical_write_path,
    bench_pure_call_overhead
);
criterion_main!(benches);
