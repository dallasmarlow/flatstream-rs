use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use flatbuffers::FlatBufferBuilder;
use flatstream::{
    AdaptiveWatermarkPolicy, DefaultFramer, NoOpPolicy, StreamSerialize, StreamWriter,
};

struct BenchData(Vec<u8>);

impl StreamSerialize for BenchData {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> flatstream::Result<()> {
        let off = builder.create_vector(&self.0);
        builder.finish(off, None);
        Ok(())
    }
}

fn benchmark_policy_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("policy_overhead");
    let small_data = BenchData(vec![0u8; 100]);

    group.throughput(Throughput::Elements(1));

    // Baseline: NoOpPolicy (Zero cost)
    group.bench_function("noop_policy", |b| {
        let mut writer = StreamWriter::builder(std::io::sink(), DefaultFramer)
            .with_policy(NoOpPolicy)
            .build();

        b.iter(|| {
            writer.write(&small_data).unwrap();
        });
    });

    // Comparison: AdaptiveWatermarkPolicy (Inactive/Steady State)
    // Configured so it never triggers (very high threshold)
    group.bench_function("adaptive_policy_inactive", |b| {
        let mut policy = AdaptiveWatermarkPolicy::default();
        policy.shrink_multiple = 1000; // Unreachable

        let mut writer = StreamWriter::builder(std::io::sink(), DefaultFramer)
            .with_policy(policy)
            .build();

        b.iter(|| {
            writer.write(&small_data).unwrap();
        });
    });

    group.finish();
}

fn benchmark_oscillation(c: &mut Criterion) {
    let mut group = c.benchmark_group("oscillation_reclamation");

    let large_data = BenchData(vec![0u8; 1024 * 1024]); // 1MB
    let small_data = BenchData(vec![0u8; 1024]); // 1KB

    // Scenario: Mixed workload with rare large messages.
    //
    // Workload per iteration (Repeated 10 times):
    // 1. Write 1 Large Message (1 MB)
    // 2. Write 1,100 Small Messages (1 KB)
    //
    // This simulates a long-running stream where a rare large event expands the buffer.
    // We compare two strategies:
    // - Unbounded (NoOp): The buffer grows to 1MB and stays there. Fast CPU, high RAM.
    // - Adaptive: The buffer shrinks back to default (16KB) after a burst of small messages.
    //   This trades a small amount of CPU (re-allocation) for significant memory savings.

    // Enough small messages to trigger the reset (1000) plus a few more (100) to use the reclaimed buffer
    let small_msg_count = 1_100;
    let cycles_per_iter = 10;

    // 1. Unbounded Growth (NoOp)
    // Result: Maximum performance. The 1MB buffer is reused for everything.
    // Trade-off: The application holds 1MB of memory indefinitely, even if 99% of traffic is small.
    group.bench_function("oscillation_noop_unbounded", |b| {
        let mut writer = StreamWriter::builder(std::io::sink(), DefaultFramer)
            .with_policy(NoOpPolicy)
            .build();

        b.iter(|| {
            for _ in 0..cycles_per_iter {
                writer.write(&large_data).unwrap();
                for _ in 0..small_msg_count {
                    writer.write(&small_data).unwrap();
                }
            }
        });
    });

    // 2. Adaptive Reclamation
    // Result: Memory efficient.
    // Logic:
    // - The large message expands capacity to 1MB.
    // - After 1000 small messages (configured below), the policy detects over-capacity.
    // - The 1MB buffer is dropped and replaced with a 16KB buffer.
    // - The remaining small messages use the 16KB buffer.
    // - The cycle repeats 10 times per iteration.
    //
    // This accurately measures the cost of 10 full grow-shrink cycles.
    group.bench_function("oscillation_adaptive_reclaim", |b| {
        let mut policy = AdaptiveWatermarkPolicy::default();
        policy.shrink_multiple = 4;
        policy.messages_to_wait = 1000; // Reclaim after 1000 small messages

        let mut writer = StreamWriter::builder(std::io::sink(), DefaultFramer)
            .with_policy(policy)
            .build();

        b.iter(|| {
            for _ in 0..cycles_per_iter {
                writer.write(&large_data).unwrap();
                for _ in 0..small_msg_count {
                    writer.write(&small_data).unwrap();
                }
            }
        });
    });

    group.finish();
}

criterion_group!(benches, benchmark_policy_overhead, benchmark_oscillation);
criterion_main!(benches);
