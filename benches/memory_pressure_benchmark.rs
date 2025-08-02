use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flatbuffers::FlatBufferBuilder;
use flatstream_rs::{self as flatstream, DefaultFramer, StreamWriter, StreamSerialize};
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
    let mut group = c.benchmark_group("Memory Pressure: 1 Large Message + 1000 Small Messages");

    let small_messages: Vec<_> = (0..1000).map(SmallMessage).collect();
    let large_message = LargeMessage(vec![0; 10 * 1024 * 1024]); // 10 MB

    // --- Benchmark 1: Simple Mode ---
    // Will allocate a large buffer for the large message and never release it,
    // causing subsequent small message writes to be less efficient.
    group.bench_function("Simple Mode", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);

            // Write one large message
            writer.write(&large_message).unwrap();

            // Write many small messages
            for msg in &small_messages {
                writer.write(msg).unwrap();
            }
            black_box(buffer);
        });
    });

    // --- Benchmark 2: Expert Mode (Multiple Builders) ---
    // Will use a separate, temporary builder for the large message,
    // allowing its memory to be freed immediately.
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
            } // `large_builder` is dropped here, freeing its ~10MB of memory

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
