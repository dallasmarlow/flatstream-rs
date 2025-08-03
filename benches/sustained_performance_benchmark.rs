use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flatbuffers::FlatBufferBuilder;
use flatstream::{self as flatstream, DefaultFramer, StreamSerialize, StreamWriter};
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
fn benchmark_sustained_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("Sustained Performance: Writing 1000 small messages");

    let small_messages: Vec<_> = (0..1000).map(SmallMessage).collect();
    let large_payload = vec![0u8; 5 * 1024 * 1024]; // 5 MB
    let large_message = LargeMessage(&large_payload);

    // --- Scenario 1: Simple Mode Simulation (Using a Bloated Builder) ---
    // We simulate the state of the simple mode *after* it has already processed
    // a large message. This is the "bad" state we want to measure.
    group.bench_function("With a Bloated Builder (Simple Mode simulation)", |b| {
        // Create the bloated builder *before* the benchmark loop.
        let mut bloated_builder = FlatBufferBuilder::new();
        large_message.serialize(&mut bloated_builder).unwrap();

        b.iter(|| {
            let temp_file = NamedTempFile::new().unwrap();
            let writer = BufWriter::new(temp_file);
            let mut stream_writer = StreamWriter::new(writer, DefaultFramer);

            // Now, we time how long it takes to write small messages
            // using this pre-bloated builder.
            for msg in &small_messages {
                bloated_builder.reset();
                msg.serialize(&mut bloated_builder).unwrap();
                stream_writer.write_finished(&mut bloated_builder).unwrap();
            }
            stream_writer.flush().unwrap();
            black_box(&stream_writer);
        });
    });

    // --- Scenario 2: Expert Mode (Using a Right-Sized Builder) ---
    // This represents the optimal pattern where small messages are always
    // handled by a small, efficient builder.
    group.bench_function("With a Right-Sized Builder (Expert Mode simulation)", |b| {
        b.iter(|| {
            let temp_file = NamedTempFile::new().unwrap();
            let writer = BufWriter::new(temp_file);
            let mut stream_writer = StreamWriter::new(writer, DefaultFramer);
            let mut small_builder = FlatBufferBuilder::new();

            // We time how long it takes to write small messages
            // using a builder that is perfectly sized for the job.
            for msg in &small_messages {
                small_builder.reset();
                msg.serialize(&mut small_builder).unwrap();
                stream_writer.write_finished(&mut small_builder).unwrap();
            }
            stream_writer.flush().unwrap();
            black_box(&stream_writer);
        });
    });

    group.finish();
}

criterion_group!(benches, benchmark_sustained_performance);
criterion_main!(benches);
