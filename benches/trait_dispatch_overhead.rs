use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::io::sink;

// A tiny message used for the practical write-path benchmark
// Note: This message actually builds a minimal FlatBuffer (creates a string and
// finishes the buffer). This ensures the Simple Mode path includes real
// serialization work inside the timed loop.
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
// Note: Its serialize() intentionally does no building work so we can separate
// the cost of the function call from the cost of constructing a FlatBuffer.
struct NoopMsg;

impl StreamSerialize for NoopMsg {
    #[inline(never)]
    fn serialize<A: flatbuffers::Allocator>(&self, _b: &mut FlatBufferBuilder<A>) -> Result<()> {
        Ok(())
    }
}

fn bench_practical_write_path(c: &mut Criterion) {
    // IMPORTANT: The two cases in this group do NOT measure the same amount of work.
    // They are intentionally asymmetric to isolate the convenience overhead of
    // Simple Mode. Specifically:
    // - Simple Mode measures: reset + serialize + frame/write
    // - Expert Mode measures: frame/write only (we pre-serialize once outside the loop)
    // The difference approximates the cost of builder.reset() + the actual work done
    // inside StreamSerialize::serialize() for this message. The function call overhead
    // (aka "trait dispatch") is separately measured in the next group and is tiny.
    let mut group = c.benchmark_group("Practical Write-Path: Simple vs. Expert Mode");

    group.bench_function("Simple Mode (with call)", |b| {
        b.iter_with_setup(
            || {
                let writer = StreamWriter::new(sink(), DefaultFramer);
                (writer, TestMsg)
            },
            |(mut writer, msg)| {
                // Timed loop: writer.write(&msg)
                // Internals (see src/writer.rs):
                // 1) self.builder.reset()
                // 2) msg.serialize(&mut self.builder)
                // 3) self.framer.frame_and_write(...)
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
                // (i.e., frame/write). This explicitly excludes serialize() from
                // the timed section.
                TestMsg.serialize(&mut builder).unwrap();
                (writer, builder)
            },
            |(mut writer, mut builder)| {
                // Timed loop: writer.write_finished(&mut builder)
                // Internals: self.framer.frame_and_write(...)
                black_box(&mut writer)
                    .write_finished(black_box(&mut builder))
                    .unwrap();
            },
        )
    });

    group.finish();
}

fn bench_pure_call_overhead(c: &mut Criterion) {
    // This group isolates the function call overhead of calling
    // StreamSerialize::serialize() itself. Both cases operate on a builder,
    // but one actually calls serialize() (which is a no-op) and the other does not.
    // The measured difference is the call overhead, not building work. In practice
    // this delta is typically sub-nanosecond on modern CPUs.
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
