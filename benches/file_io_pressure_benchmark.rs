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

fn benchmark_file_io_pressure(c: &mut Criterion) {
    // ---
    // # Benchmark Purpose: File I/O Pressure Under Mixed Sizes
    //
    // Central question: How does the write path behave, including filesystem effects,
    // when a run contains one very large message followed by many small messages?
    //
    // Design: Tempfile per iteration, BufWriter to reflect best practices. Compare
    // Simple vs Expert (multiple builders) patterns. The large message is prebuilt
    // once outside the loop to avoid generation costs in timing.
    //
    // Notes: Results are influenced by OS page cache and disk characteristics. The
    // goal is qualitative comparison between patterns rather than absolute IOPS.
    // ---
    let mut group = c.benchmark_group("File I/O Pressure: 1 Large (10MB) + 1000 Small Messages");

    let small_messages: Vec<_> = (0..1000).map(SmallMessage).collect();
    // Create the large message payload once to avoid re-allocation in the benchmark loop
    let large_payload = vec![0u8; 10 * 1024 * 1024];
    let large_message = LargeMessage(&large_payload);

    // --- Benchmark 1: Simple Mode ---
    // This mode will suffer from memory bloat. The internal builder will grow to 10MB
    // and stay that size for all subsequent small messages, leading to inefficient
    // memory management and more work for the OS and allocator.
    group.bench_function("Simple Mode", |b| {
        b.iter(|| {
            // Use a tempfile to ensure each run is a realistic file write
            let temp_file = NamedTempFile::new().unwrap();
            let writer = BufWriter::new(temp_file);
            let mut stream_writer = StreamWriter::new(writer, DefaultFramer);

            // First write triggers the large allocation
            stream_writer.write(&large_message).unwrap();

            // The internal builder is now bloated for all subsequent small writes
            for msg in &small_messages {
                stream_writer.write(msg).unwrap();
            }
            // Ensure data is flushed to disk
            stream_writer.flush().unwrap();
            black_box(stream_writer);
        });
    });

    // --- Benchmark 2: Expert Mode (Multiple Builders) ---
    // This is the efficient pattern. The large builder is temporary and its memory
    // is freed immediately. The small builder stays small and efficient.
    group.bench_function("Expert Mode (Multiple Builders)", |b| {
        b.iter(|| {
            let temp_file = NamedTempFile::new().unwrap();
            let writer = BufWriter::new(temp_file);
            let mut stream_writer = StreamWriter::new(writer, DefaultFramer);
            let mut small_builder = FlatBufferBuilder::new();

            // Use a temporary, scoped builder for the large message
            {
                let mut large_builder = FlatBufferBuilder::new();
                large_message.serialize(&mut large_builder).unwrap();
                stream_writer.write_finished(&mut large_builder).unwrap();
            } // ~10MB of memory is freed here

            // The small_builder remains small and efficient
            for msg in &small_messages {
                small_builder.reset();
                msg.serialize(&mut small_builder).unwrap();
                stream_writer.write_finished(&mut small_builder).unwrap();
            }
            stream_writer.flush().unwrap();
            black_box(stream_writer);
        });
    });

    group.finish();
}

criterion_group!(benches, benchmark_file_io_pressure);
criterion_main!(benches);
