use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use flatbuffers::FlatBufferBuilder;
use flatstream::policy::ReclamationReason;
use flatstream::{
    AdaptiveWatermarkPolicy, DefaultFramer, MemoryPolicy, NoOpPolicy, StreamSerialize, StreamWriter,
};

/// A never-firing policy with a baseline of 1 byte: keeps the writer's
/// steady-state gate open so the bench measures the boxed dispatch itself,
/// not the gate. (`NoOpPolicy`'s default 16 KiB baseline would close the gate
/// for small builders and skip the call entirely.)
struct GateOpenNoOp;

impl MemoryPolicy for GateOpenNoOp {
    fn should_reset(&mut self, _: usize, _: usize) -> Option<ReclamationReason> {
        None
    }

    fn baseline_capacity(&self) -> usize {
        1
    }
}

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

    // Baseline: no policy installed (the default). Measures the cost of the
    // single not-taken branch in write().
    group.bench_function("no_policy", |b| {
        let mut writer = StreamWriter::new(std::io::sink(), DefaultFramer);

        b.iter(|| {
            writer.write(&small_data).unwrap();
        });
    });

    // Comparison: a no-op policy installed. Measures the boxed-policy dispatch
    // cost on top of the baseline (one indirect call per message); GateOpenNoOp's
    // 1-byte baseline keeps the gate open so the call actually happens.
    group.bench_function("noop_policy", |b| {
        let mut writer =
            StreamWriter::new(std::io::sink(), DefaultFramer).with_memory_policy(GateOpenNoOp);

        b.iter(|| {
            writer.write(&small_data).unwrap();
        });
    });

    // Comparison: AdaptiveWatermarkPolicy (Inactive/Steady State)
    //
    // We configure the threshold to be unreachable (1000x message size).
    // This prevents any resets from occurring.
    //
    // GOAL: Measure the pure CPU overhead of the policy's book-keeping logic
    // (tracking sizes, checking thresholds) to prove it is negligible when
    // not actively reclaiming memory.
    group.bench_function("adaptive_policy_inactive", |b| {
        // Ratio of 1000 is unreachable by design: measures pure bookkeeping.
        // with_baseline(1) keeps the steady-state gate open (see above).
        let policy = AdaptiveWatermarkPolicy::new(1000, 5).with_baseline(1);

        let mut writer =
            StreamWriter::new(std::io::sink(), DefaultFramer).with_memory_policy(policy);

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
        let mut writer =
            StreamWriter::new(std::io::sink(), DefaultFramer).with_memory_policy(NoOpPolicy);

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
        // Reclaim after 1000 consecutive small messages
        let policy = AdaptiveWatermarkPolicy::new(4, 1000);

        let mut writer =
            StreamWriter::new(std::io::sink(), DefaultFramer).with_memory_policy(policy);

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
