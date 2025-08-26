use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flatbuffers::FlatBufferBuilder;
use flatstream::{self as flatstream, DefaultFramer, StreamSerialize, StreamWriter};
use std::io::Cursor;

// --- Message Types ---

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

// --- Benchmark Function ---

fn benchmark_memory_pressure(c: &mut Criterion) {
    // ---
    // # Benchmark Purpose: Memory Bloat Under Pressure
    //
    // Central question: How does the writer's memory usage and throughput behave when
    // a stream mixes extremely large and very small messages?
    //
    // Design: We write one large message (10 MiB) followed by many tiny messages.
    // In Simple Mode, the internal FlatBufferBuilder will grow to accommodate the
    // large message and retain that capacity, causing memory bloat for subsequent
    // small messages. In Expert Mode we use a separate, temporary builder for the
    // large message, then a dedicated small builder for the rest.
    //
    // Expected takeaway: Expert Mode avoids memory bloat and may be faster in this
    // mixed-size workload by keeping the small-builder footprint minimal.
    // ---
    let mut group = c.benchmark_group("Memory Pressure: 1 Large Message + 1000 Small Messages");

    let small_messages: Vec<_> = (0..1000).map(SmallMessage).collect();
    let large_message = LargeMessage(vec![0; 10 * 1024 * 1024]); // 10 MB

    // --- Benchmark 1: Simple Mode ---
    // Measures: For each write, the writer reuses its single internal builder.
    // Consequence: After the first 10 MiB message, the builder keeps that large
    // capacity. All subsequent tiny messages pay the bloat cost in memory.
    group.bench_function("Simple Mode", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);

            // STEP 1: Write one large message (forces builder to grow to ~10 MiB)
            writer.write(&large_message).unwrap();

            // STEP 2: Write many small messages with the same (bloated) builder
            for msg in &small_messages {
                writer.write(msg).unwrap();
            }
            black_box(buffer);
        });
    });

    // --- Benchmark 2: Expert Mode (Multiple Builders) ---
    // Measures: Use a scoped, temporary builder for the single large message, and
    // reuse a separate small builder for all tiny messages.
    // Consequence: The large builder is dropped after use, so its memory is freed;
    // the small builder remains compact and efficient for the rest of the stream.
    group.bench_function("Expert Mode (Multiple Builders)", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
            let mut small_builder = FlatBufferBuilder::new();

            // Use a separate, temporary builder for the large message
            {
                let mut large_builder = FlatBufferBuilder::new();
                large_builder.reset();
                large_message.serialize(&mut large_builder).unwrap();
                writer.write_finished(&mut large_builder).unwrap();
            } // `large_builder` is dropped here, freeing its ~10 MiB of memory

            // Use the small builder for all subsequent messages
            for msg in &small_messages {
                small_builder.reset();
                msg.serialize(&mut small_builder).unwrap();
                writer.write_finished(&mut small_builder).unwrap();
            }
            black_box(buffer);
        });
    });

    group.finish();
}

criterion_group!(benches, benchmark_memory_pressure);
criterion_main!(benches);
