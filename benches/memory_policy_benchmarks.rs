use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use flatbuffers::FlatBufferBuilder;
use flatstream::{
    AdaptiveWatermarkPolicy, DefaultFramer, MemoryPolicy, NoOpPolicy, StreamSerialize, StreamWriter,
};
use std::io::Sink;

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
        let policy = AdaptiveWatermarkPolicy {
            shrink_multiple: 1000, // Unreachable
            ..Default::default()
        };
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
    let small_data = BenchData(vec![0u8; 1024]);        // 1KB

    // Scenario: 1 Large, 10 Small (repeated)
    // This stresses the reclamation logic.
    
    // 1. Unbounded Growth (NoOp)
    // Memory usage will stay high (1MB+), but CPU cost is low (no re-allocs).
    group.bench_function("oscillation_noop_unbounded", |b| {
        let mut writer = StreamWriter::builder(std::io::sink(), DefaultFramer)
            .with_policy(NoOpPolicy)
            .build();
            
        b.iter(|| {
            writer.write(&large_data).unwrap();
            for _ in 0..10 {
                writer.write(&small_data).unwrap();
            }
        });
    });

    // 2. Adaptive Reclamation
    // Memory usage drops after small messages, but incurs re-alloc cost on next large message.
    group.bench_function("oscillation_adaptive_reclaim", |b| {
        let policy = AdaptiveWatermarkPolicy {
            shrink_multiple: 4,
            messages_to_wait: 5, // Reclaim after 5 small messages
            ..Default::default()
        };
        let mut writer = StreamWriter::builder(std::io::sink(), DefaultFramer)
            .with_policy(policy)
            .build();
            
        b.iter(|| {
            writer.write(&large_data).unwrap();
            for _ in 0..10 {
                writer.write(&small_data).unwrap();
            }
        });
    });

    group.finish();
}

criterion_group!(benches, benchmark_policy_overhead, benchmark_oscillation);
criterion_main!(benches);

