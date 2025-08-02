use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flatbuffers::FlatBufferBuilder;
use flatstream::{self as flatstream, DefaultFramer, StreamWriter, StreamSerialize};
use std::io::BufWriter;
use tempfile::NamedTempFile;

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

struct LargeMessage<'a>(&'a [u8]);

impl<'a> StreamSerialize for LargeMessage<'a> {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> flatstream::Result<()> {
        let vec = builder.create_vector(self.0);
        builder.finish(vec, None);
        Ok(())
    }
}

// --- Benchmark Function ---

fn benchmark_long_running_agent(c: &mut Criterion) {
    let mut group = c.benchmark_group("Long-Running Agent: 10 cycles of [1 Large (5MB) + 1000 Small]");

    // Pre-allocate data to avoid measuring allocation of the test data itself
    let small_messages: Vec<_> = (0..1000).map(SmallMessage).collect();
    let large_payload = vec![0u8; 5 * 1024 * 1024]; // 5 MB
    let large_message = LargeMessage(&large_payload);
    const CYCLES: usize = 10;

    // --- Benchmark 1: Simple Mode ---
    // The internal builder will grow to 5MB on the first cycle and the OS will have to
    // manage this large, inefficient buffer for the remaining 9,990 small writes.
    group.bench_function("Simple Mode", |b| {
        b.iter(|| {
            let temp_file = NamedTempFile::new().unwrap();
            let writer = BufWriter::new(temp_file);
            let mut stream_writer = StreamWriter::new(writer, DefaultFramer);

            for _ in 0..CYCLES {
                // The first write in the first cycle bloats the internal builder
                stream_writer.write(&large_message).unwrap();

                // All subsequent small writes in all cycles use the bloated builder
                for msg in &small_messages {
                    stream_writer.write(msg).unwrap();
                }
            }
            stream_writer.flush().unwrap();
            black_box(stream_writer);
        });
    });

    // --- Benchmark 2: Expert Mode (Multiple Builders) ---
    // The 5MB builder is created and dropped in each cycle, freeing the memory.
    // The small builder stays small and fast. This is far more efficient.
    group.bench_function("Expert Mode (Multiple Builders)", |b| {
        b.iter(|| {
            let temp_file = NamedTempFile::new().unwrap();
            let writer = BufWriter::new(temp_file);
            let mut stream_writer = StreamWriter::new(writer, DefaultFramer);
            let mut small_builder = FlatBufferBuilder::new();

            for _ in 0..CYCLES {
                // The large builder is temporary and its memory is reclaimed after each cycle
                {
                    let mut large_builder = FlatBufferBuilder::new();
                    large_message.serialize(&mut large_builder).unwrap();
                    stream_writer.write_finished(&mut large_builder).unwrap();
                } // ~5MB of memory is freed here, every cycle

                // The small_builder remains efficient
                for msg in &small_messages {
                    small_builder.reset();
                    msg.serialize(&mut small_builder).unwrap();
                    stream_writer.write_finished(&mut small_builder).unwrap();
                }
            }
            stream_writer.flush().unwrap();
            black_box(stream_writer);
        });
    });

    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10); // Use a smaller sample size for this long-running bench
    targets = benchmark_long_running_agent
}
criterion_main!(benches);