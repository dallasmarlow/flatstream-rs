use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flatbuffers::FlatBufferBuilder;
use flatstream_rs::{self as flatstream, DefaultFramer, StreamWriter, StreamSerialize};
use std::io::Cursor;

// --- Message Types ---

// A small, frequent message type (e.g., a heartbeat or a simple event)
struct SmallMessage(u32);

impl StreamSerialize for SmallMessage {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> flatstream::Result<()> {
        let s = builder.create_string(&self.0.to_string());
        builder.finish(s, None);
        Ok(())
    }
}

// A large, infrequent message type (e.g., a data dump or a file chunk)
struct LargeMessage(Vec<u8>);

impl StreamSerialize for LargeMessage {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> flatstream::Result<()> {
        let vec = builder.create_vector(&self.0);
        builder.finish(vec, None);
        Ok(())
    }
}

enum MixedMessage {
    Small(SmallMessage),
    Large(LargeMessage),
}

// --- Benchmark Function ---

fn benchmark_real_world_scenario(c: &mut Criterion) {
    let mut group = c.benchmark_group("Real-World Performance: Mixed Message Sizes");

    // Create a realistic workload: 1000 small messages and 10 large messages (1MB each)
    let mut messages = Vec::new();
    for i in 0..1000 {
        messages.push(MixedMessage::Small(SmallMessage(i)));
        if i % 100 == 0 {
            messages.push(MixedMessage::Large(LargeMessage(vec![0; 1_000_000])));
        }
    }

    // --- Benchmark 1: Simple Mode ---
    // This will demonstrate the memory bloat issue.
    group.bench_function("Simple Mode", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
            for msg in &messages {
                match msg {
                    MixedMessage::Small(s) => writer.write(s).unwrap(),
                    MixedMessage::Large(l) => writer.write(l).unwrap(),
                }
            }
            black_box(buffer);
        });
    });

    // --- Benchmark 2: Expert Mode with a Single Builder ---
    // This shows that just using expert mode is not enough; you need to use the right pattern.
    group.bench_function("Expert Mode (Single Builder)", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
            let mut builder = FlatBufferBuilder::new();
            for msg in &messages {
                builder.reset();
                match msg {
                    MixedMessage::Small(s) => s.serialize(&mut builder).unwrap(),
                    MixedMessage::Large(l) => l.serialize(&mut builder).unwrap(),
                }
                writer.write_finished(&mut builder).unwrap();
            }
            black_box(buffer);
        });
    });

    // --- Benchmark 3: Expert Mode with Multiple Builders ---
    // This is the optimal pattern for this scenario.
    group.bench_function("Expert Mode (Multiple Builders)", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
            let mut small_builder = FlatBufferBuilder::new();
            let mut large_builder = FlatBufferBuilder::new();
            for msg in &messages {
                match msg {
                    MixedMessage::Small(s) => {
                        small_builder.reset();
                        s.serialize(&mut small_builder).unwrap();
                        writer.write_finished(&mut small_builder).unwrap();
                    }
                    MixedMessage::Large(l) => {
                        large_builder.reset();
                        l.serialize(&mut large_builder).unwrap();
                        writer.write_finished(&mut large_builder).unwrap();
                    }
                }
            }
            black_box(buffer);
        });
    });

    group.finish();
}

criterion_group!(benches, benchmark_real_world_scenario);
criterion_main!(benches);