use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flatbuffers::FlatBufferBuilder;
use flatstream::{self as flatstream, DefaultFramer, StreamSerialize, StreamWriter};
use std::io::BufWriter;
use tempfile::NamedTempFile;

// --- Message Types ---

// Represents a small, common message (e.g., heartbeat, simple event)
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

// Represents a medium-sized, less frequent message (e.g., a batch of events)
struct MediumMessage<'a>(&'a [u8]);
impl<'a> StreamSerialize for MediumMessage<'a> {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> flatstream::Result<()> {
        let vec = builder.create_vector(self.0);
        builder.finish(vec, None);
        Ok(())
    }
}


// Represents a large, rare message (e.g., a file chunk, a large state dump)
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

// Enum to represent our mixed workload
enum MixedMessage<'a> {
    Small(SmallMessage),
    Medium(MediumMessage<'a>),
    Large(LargeMessage<'a>),
}

// FIX: Implement StreamSerialize for the MixedMessage enum.
// This allows the `Simple Mode` benchmark to call `writer.write()` directly on it.
impl<'a> StreamSerialize for MixedMessage<'a> {
    fn serialize<A: flatbuffers::Allocator>(&self, builder: &mut FlatBufferBuilder<A>) -> flatstream::Result<()> {
        match self {
            MixedMessage::Small(s) => s.serialize(builder),
            MixedMessage::Medium(m) => m.serialize(builder),
            MixedMessage::Large(l) => l.serialize(builder),
        }
    }
}


impl<'a> MixedMessage<'a> {
    // Helper to get an approximate size for routing to the correct buffer
    fn size_hint(&self) -> usize {
        match self {
            MixedMessage::Small(_) => 64, // Small messages are under 64 bytes
            MixedMessage::Medium(m) => m.0.len(),
            MixedMessage::Large(l) => l.0.len(),
        }
    }
}


// --- Benchmark Function ---

fn benchmark_long_running_agent(c: &mut Criterion) {
    let mut group = c.benchmark_group("Long-Running Agent: Mixed Workload with Large Messages");

    // Pre-allocate data to avoid measuring test data allocation itself
    let medium_payload = vec![0u8; 64 * 1024]; // 64 KB
    let large_payload = vec![0u8; 5 * 1024 * 1024]; // 5 MB
    
    // Create a more realistic, mixed workload
    let mut workload = Vec::new();
    for i in 0..1000 {
        workload.push(MixedMessage::Small(SmallMessage(i)));
        if i % 100 == 0 {
            workload.push(MixedMessage::Medium(MediumMessage(&medium_payload)));
        }
    }
    // Add one large message to the workload
    workload.push(MixedMessage::Large(LargeMessage(&large_payload)));


    // --- Benchmark 1: Simple Mode (Baseline) ---
    // Will suffer from memory bloat after seeing the large message.
    group.bench_function("Simple Mode (Reuses Large Buffer)", |b| {
        b.iter(|| {
            let temp_file = NamedTempFile::new().unwrap();
            let writer = BufWriter::new(temp_file);
            let mut stream_writer = StreamWriter::new(writer, DefaultFramer);

            for msg in &workload {
                stream_writer.write(msg).unwrap();
            }
            stream_writer.flush().unwrap();
            black_box(stream_writer);
        });
    });

    // --- Benchmark 2: Expert Mode (Memory Efficient - Re-allocates) ---
    // Prioritizes memory but is slow due to repeated large allocations.
    group.bench_function("Expert Mode (Memory Efficient - Re-allocates)", |b| {
        b.iter(|| {
            let temp_file = NamedTempFile::new().unwrap();
            let writer = BufWriter::new(temp_file);
            let mut stream_writer = StreamWriter::new(writer, DefaultFramer);
            let mut small_builder = FlatBufferBuilder::new();
            let mut medium_builder = FlatBufferBuilder::new();

            for msg in &workload {
                 match msg {
                    MixedMessage::Small(s) => {
                        small_builder.reset();
                        s.serialize(&mut small_builder).unwrap();
                        stream_writer.write_finished(&mut small_builder).unwrap();
                    }
                    MixedMessage::Medium(m) => {
                        medium_builder.reset();
                        m.serialize(&mut medium_builder).unwrap();
                        stream_writer.write_finished(&mut medium_builder).unwrap();
                    }
                    MixedMessage::Large(l) => {
                        // Creates and drops a temporary builder for the large message
                        let mut large_builder = FlatBufferBuilder::new();
                        l.serialize(&mut large_builder).unwrap();
                        stream_writer.write_finished(&mut large_builder).unwrap();
                    }
                }
            }
            stream_writer.flush().unwrap();
            black_box(stream_writer);
        });
    });
    
    // --- NEW: Benchmark 3: Expert Mode (Adaptive Tiered Buffers) ---
    // This implements your suggested pattern: a tiered set of reusable buffers.
    group.bench_function("Expert Mode (Adaptive Tiered Buffers)", |b| {
        b.iter(|| {
            let temp_file = NamedTempFile::new().unwrap();
            let writer = BufWriter::new(temp_file);
            let mut stream_writer = StreamWriter::new(writer, DefaultFramer);
            
            // Create a pool of builders for different size tiers.
            let mut small_builder = FlatBufferBuilder::with_capacity(1024); // For messages < 1KB
            let mut medium_builder = FlatBufferBuilder::with_capacity(128 * 1024); // For messages < 128KB
            
            for msg in &workload {
                // Route the message to the appropriate builder based on its size.
                match msg.size_hint() {
                    // Small messages go to the small builder
                    s if s <= 1024 => {
                        small_builder.reset();
                        msg.serialize(&mut small_builder).unwrap();
                        stream_writer.write_finished(&mut small_builder).unwrap();
                    }
                    // Medium messages go to the medium builder
                    s if s <= 128 * 1024 => {
                        medium_builder.reset();
                        msg.serialize(&mut medium_builder).unwrap();
                        stream_writer.write_finished(&mut medium_builder).unwrap();
                    }
                    // Large messages get a temporary builder that is reclaimed.
                    _ => {
                        let mut large_builder = FlatBufferBuilder::new();
                        msg.serialize(&mut large_builder).unwrap();
                        stream_writer.write_finished(&mut large_builder).unwrap();
                        // The large buffer is dropped here, reclaiming memory immediately.
                    }
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
    config = Criterion::default().sample_size(10);
    targets = benchmark_long_running_agent
}
criterion_main!(benches);
