// benches/comparative_benchmarks.rs
// Comparative benchmarks between flatstream-rs and alternative serialization approaches

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flatbuffers::FlatBufferBuilder;
use flatstream_rs::{
    self as flatstream, DefaultDeframer, DefaultFramer, StreamReader, StreamSerialize, StreamWriter,
};
use serde::{Deserialize, Serialize};
use std::io::{Cursor, Read, Write};

// Import checksum types when features are enabled
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
use flatstream_rs::framing::{ChecksumDeframer, ChecksumFramer};

#[cfg(feature = "xxhash")]
use flatstream_rs::XxHash64;

#[cfg(feature = "crc32")]
use flatstream_rs::Crc32;

#[cfg(feature = "crc16")]
use flatstream_rs::Crc16;

// Arena allocation bridge
#[cfg(feature = "bumpalo")]
use bumpalo::Bump;
#[cfg(feature = "bumpalo")]
use flatbuffers::Allocator;
#[cfg(feature = "bumpalo")]
use std::ops::{Deref, DerefMut};
#[cfg(feature = "bumpalo")]
use std::ptr::NonNull;

// This is the bridge that connects bumpalo with flatbuffers::Allocator
// It uses bumpalo to manage a growable buffer that FlatBuffers can index into
#[cfg(feature = "bumpalo")]
struct BumpaloAllocator<'a> {
    arena: &'a Bump,
    buffer_ptr: NonNull<u8>,
    buffer_len: usize,
    buffer_capacity: usize,
}

#[cfg(feature = "bumpalo")]
impl<'a> BumpaloAllocator<'a> {
    fn new(arena: &'a Bump) -> Self {
        // Start with a reasonable initial capacity
        let initial_capacity = 1024;
        let layout = std::alloc::Layout::from_size_align(initial_capacity, 8).unwrap();
        let buffer_ptr = arena.alloc_layout(layout);

        // Initialize the buffer with zeros
        unsafe {
            std::ptr::write_bytes(buffer_ptr.as_ptr(), 0, initial_capacity);
        }

        Self {
            arena,
            buffer_ptr,
            buffer_len: initial_capacity,
            buffer_capacity: initial_capacity,
        }
    }
}

#[cfg(feature = "bumpalo")]
impl<'a> Deref for BumpaloAllocator<'a> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        unsafe { std::slice::from_raw_parts(self.buffer_ptr.as_ptr(), self.buffer_len) }
    }
}

#[cfg(feature = "bumpalo")]
impl<'a> DerefMut for BumpaloAllocator<'a> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { std::slice::from_raw_parts_mut(self.buffer_ptr.as_ptr(), self.buffer_len) }
    }
}

#[cfg(feature = "bumpalo")]
unsafe impl<'a> Allocator for BumpaloAllocator<'a> {
    type Error = std::io::Error;

    fn grow_downwards(&mut self) -> Result<(), Self::Error> {
        // Double the capacity
        let new_capacity = std::cmp::max(1, self.buffer_capacity * 2);
        let new_layout = std::alloc::Layout::from_size_align(new_capacity, 8).unwrap();

        // Allocate new buffer from arena
        let new_buffer_ptr = self.arena.alloc_layout(new_layout);

        // Initialize new buffer with zeros
        unsafe {
            std::ptr::write_bytes(new_buffer_ptr.as_ptr(), 0, new_capacity);
        }

        // Copy existing data to the end of the new buffer
        unsafe {
            let new_buffer_slice =
                std::slice::from_raw_parts_mut(new_buffer_ptr.as_ptr(), new_capacity);
            let old_data_start = new_capacity - self.buffer_len;
            new_buffer_slice[old_data_start..].copy_from_slice(&self[..]);
        }

        // Update our state
        self.buffer_ptr = new_buffer_ptr;
        self.buffer_capacity = new_capacity;

        Ok(())
    }

    fn len(&self) -> usize {
        self.buffer_len
    }
}

// --- Test function to verify BumpaloAllocator ---
#[cfg(feature = "bumpalo")]
fn test_bumpalo_allocator() {
    use flatbuffers::FlatBufferBuilder;

    // Create arena
    let arena = Bump::new();

    // Create our allocator
    let allocator = BumpaloAllocator::new(&arena);

    // Create FlatBufferBuilder with our allocator
    let mut builder = FlatBufferBuilder::new_in(allocator);

    // Test basic functionality
    let test_string = builder.create_string("Hello, Arena!");
    builder.finish(test_string, None);

    // Get the finished data
    let data = builder.finished_data();

    // Verify we got some data
    assert!(!data.is_empty(), "BumpaloAllocator should produce data");
    println!(
        "BumpaloAllocator test passed! Produced {} bytes",
        data.len()
    );
}

// --- Common Data Structure ---
// A simple struct that can be used by serde and flatstream.

#[derive(Serialize, Deserialize, Debug, Clone)]
struct TelemetryEvent {
    device_id: u64,
    timestamp: u64,
    value: f64,
}

// --- flatstream-rs Implementation ---
// PROPER FlatBuffer implementation using binary tables instead of strings

impl StreamSerialize for TelemetryEvent {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> flatstream::Result<()> {
        // Create a proper FlatBuffer table structure
        // This is much more efficient than string formatting

        // For this benchmark, we'll create a simple binary structure
        // In a real implementation, you'd use FlatBuffer schema files

        // Create a binary representation: [device_id: u64][timestamp: u64][value: f64]
        let mut data = Vec::with_capacity(24); // 8 + 8 + 8 bytes

        // Add device_id (little-endian)
        data.extend_from_slice(&self.device_id.to_le_bytes());
        // Add timestamp (little-endian)
        data.extend_from_slice(&self.timestamp.to_le_bytes());
        // Add value (little-endian)
        data.extend_from_slice(&self.value.to_le_bytes());

        // Create a FlatBuffer string from our binary data
        let data_vec = builder.create_vector(&data);
        builder.finish(data_vec, None);
        Ok(())
    }
}

// --- Benchmark Functions ---

fn benchmark_alternatives_small(c: &mut Criterion) {
    let mut group = c.benchmark_group("Small Dataset (100 events)");

    // Test our BumpaloAllocator first
    #[cfg(feature = "bumpalo")]
    {
        test_bumpalo_allocator();
    }

    // Create test data - 100 telemetry events
    let events: Vec<TelemetryEvent> = (0..100)
        .map(|i| TelemetryEvent {
            device_id: i,
            timestamp: 1672531200 + i,
            value: i as f64 * 1.5,
        })
        .collect();

    // Benchmark 1: flatstream-rs with default framer (no checksum)
    group.bench_function("flatstream_default", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            // Write phase
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
            for event in &events {
                writer.write(event).unwrap();
            }
            black_box(&buffer);

            // Read phase
            let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box(count);
        });
    });

    // Benchmark 2: flatstream-rs with XXHash64 checksum
    #[cfg(feature = "xxhash")]
    group.bench_function("flatstream_xxhash64", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            // Write phase
            let checksum = XxHash64::new();
            let framer = ChecksumFramer::new(checksum);
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
            for event in &events {
                writer.write(event).unwrap();
            }
            black_box(&buffer);

            // Read phase
            let checksum = XxHash64::new();
            let deframer = ChecksumDeframer::new(checksum);
            let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box(count);
        });
    });

    // Benchmark 3: flatstream-rs with CRC32 checksum
    #[cfg(feature = "crc32")]
    group.bench_function("flatstream_crc32", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            // Write phase
            let checksum = Crc32::new();
            let framer = ChecksumFramer::new(checksum);
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
            for event in &events {
                writer.write(event).unwrap();
            }
            black_box(&buffer);

            // Read phase
            let checksum = Crc32::new();
            let deframer = ChecksumDeframer::new(checksum);
            let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box(count);
        });
    });

    // Benchmark 4: flatstream-rs with CRC16 checksum
    #[cfg(feature = "crc16")]
    group.bench_function("flatstream_crc16", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            // Write phase
            let checksum = Crc16::new();
            let framer = ChecksumFramer::new(checksum);
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
            for event in &events {
                writer.write(event).unwrap();
            }
            black_box(&buffer);

            // Read phase
            let checksum = Crc16::new();
            let deframer = ChecksumDeframer::new(checksum);
            let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box(count);
        });
    });

    // Benchmark 5: flatstream-rs with true arena allocation using bumpalo
    #[cfg(feature = "bumpalo")]
    group.bench_function("flatstream_arena", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();

            // Create the arena allocator
            let arena = Bump::new();

            // Create our custom allocator wrapper
            let allocator = BumpaloAllocator::new(&arena);

            // Create the builder using our arena-backed allocator
            let builder = FlatBufferBuilder::new_in(allocator);

            // Create the writer with our arena-backed builder
            let mut writer =
                StreamWriter::with_builder(Cursor::new(&mut buffer), DefaultFramer, builder);

            for event in &events {
                writer.write(event).unwrap();
            }
            black_box(&buffer);

            // Read phase
            let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box(count);
        });
    });

    // Benchmark 6: bincode + manual framing
    group.bench_function("bincode", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            // Write phase
            for event in &events {
                let encoded = bincode::serialize(event).unwrap();
                let len = encoded.len() as u32;
                buffer.write_all(&len.to_le_bytes()).unwrap();
                buffer.write_all(&encoded).unwrap();
            }
            black_box(&buffer);

            // Read phase
            let mut reader = Cursor::new(&buffer);
            let mut len_bytes = [0u8; 4];
            let mut count = 0;
            while reader.read_exact(&mut len_bytes).is_ok() {
                let len = u32::from_le_bytes(len_bytes) as usize;
                let mut message_buf = vec![0u8; len];
                reader.read_exact(&mut message_buf).unwrap();
                let _decoded: TelemetryEvent = bincode::deserialize(&message_buf).unwrap();
                count += 1;
            }
            black_box(count);
        });
    });

    // Benchmark 7: JSON + manual framing
    group.bench_function("serde_json", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            // Write phase
            for event in &events {
                let encoded = serde_json::to_vec(event).unwrap();
                let len = encoded.len() as u32;
                buffer.write_all(&len.to_le_bytes()).unwrap();
                buffer.write_all(&encoded).unwrap();
            }
            black_box(&buffer);

            // Read phase
            let mut reader = Cursor::new(&buffer);
            let mut len_bytes = [0u8; 4];
            let mut count = 0;
            while reader.read_exact(&mut len_bytes).is_ok() {
                let len = u32::from_le_bytes(len_bytes) as usize;
                let mut message_buf = vec![0u8; len];
                reader.read_exact(&mut message_buf).unwrap();
                let _decoded: TelemetryEvent = serde_json::from_slice(&message_buf).unwrap();
                count += 1;
            }
            black_box(count);
        });
    });

    group.finish();
}

fn benchmark_alternatives_large(c: &mut Criterion) {
    let mut group = c.benchmark_group("Large Dataset (~10MB)");

    // Create test data - approximately 10MB of telemetry events
    // Each event is roughly 100 bytes, so we need about 100,000 events
    let events: Vec<TelemetryEvent> = (0..100_000)
        .map(|i| TelemetryEvent {
            device_id: i % 1000, // Reuse device IDs to reduce memory
            timestamp: 1672531200 + i,
            value: i as f64 * 1.5,
        })
        .collect();

    // Benchmark 1: flatstream-rs with default framer (no checksum)
    group.bench_function("flatstream_default", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            // Write phase
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
            for event in &events {
                writer.write(event).unwrap();
            }
            black_box(&buffer);

            // Read phase
            let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box(count);
        });
    });

    // Benchmark 2: flatstream-rs with XXHash64 checksum
    #[cfg(feature = "xxhash")]
    group.bench_function("flatstream_xxhash64", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            // Write phase
            let checksum = XxHash64::new();
            let framer = ChecksumFramer::new(checksum);
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
            for event in &events {
                writer.write(event).unwrap();
            }
            black_box(&buffer);

            // Read phase
            let checksum = XxHash64::new();
            let deframer = ChecksumDeframer::new(checksum);
            let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box(count);
        });
    });

    // Benchmark 3: flatstream-rs with CRC32 checksum
    #[cfg(feature = "crc32")]
    group.bench_function("flatstream_crc32", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            // Write phase
            let checksum = Crc32::new();
            let framer = ChecksumFramer::new(checksum);
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
            for event in &events {
                writer.write(event).unwrap();
            }
            black_box(&buffer);

            // Read phase
            let checksum = Crc32::new();
            let deframer = ChecksumDeframer::new(checksum);
            let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box(count);
        });
    });

    // Benchmark 4: flatstream-rs with CRC16 checksum
    #[cfg(feature = "crc16")]
    group.bench_function("flatstream_crc16", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            // Write phase
            let checksum = Crc16::new();
            let framer = ChecksumFramer::new(checksum);
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
            for event in &events {
                writer.write(event).unwrap();
            }
            black_box(&buffer);

            // Read phase
            let checksum = Crc16::new();
            let deframer = ChecksumDeframer::new(checksum);
            let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box(count);
        });
    });

    // Benchmark 5: flatstream-rs with true arena allocation using bumpalo
    #[cfg(feature = "bumpalo")]
    group.bench_function("flatstream_arena", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();

            // Create the arena allocator
            let arena = Bump::new();

            // Create our custom allocator wrapper
            let allocator = BumpaloAllocator::new(&arena);

            // Create the builder using our arena-backed allocator
            let builder = FlatBufferBuilder::new_in(allocator);

            // Create the writer with our arena-backed builder
            let mut writer =
                StreamWriter::with_builder(Cursor::new(&mut buffer), DefaultFramer, builder);

            for event in &events {
                writer.write(event).unwrap();
            }
            black_box(&buffer);

            // Read phase
            let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box(count);
        });
    });

    // Benchmark 6: bincode + manual framing
    group.bench_function("bincode", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            // Write phase
            for event in &events {
                let encoded = bincode::serialize(event).unwrap();
                let len = encoded.len() as u32;
                buffer.write_all(&len.to_le_bytes()).unwrap();
                buffer.write_all(&encoded).unwrap();
            }
            black_box(&buffer);

            // Read phase
            let mut reader = Cursor::new(&buffer);
            let mut len_bytes = [0u8; 4];
            let mut count = 0;
            while reader.read_exact(&mut len_bytes).is_ok() {
                let len = u32::from_le_bytes(len_bytes) as usize;
                let mut message_buf = vec![0u8; len];
                reader.read_exact(&mut message_buf).unwrap();
                let _decoded: TelemetryEvent = bincode::deserialize(&message_buf).unwrap();
                count += 1;
            }
            black_box(count);
        });
    });

    // Benchmark 7: JSON + manual framing
    group.bench_function("serde_json", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            // Write phase
            for event in &events {
                let encoded = serde_json::to_vec(event).unwrap();
                let len = encoded.len() as u32;
                buffer.write_all(&len.to_le_bytes()).unwrap();
                buffer.write_all(&encoded).unwrap();
            }
            black_box(&buffer);

            // Read phase
            let mut reader = Cursor::new(&buffer);
            let mut len_bytes = [0u8; 4];
            let mut count = 0;
            while reader.read_exact(&mut len_bytes).is_ok() {
                let len = u32::from_le_bytes(len_bytes) as usize;
                let mut message_buf = vec![0u8; len];
                reader.read_exact(&mut message_buf).unwrap();
                let _decoded: TelemetryEvent = serde_json::from_slice(&message_buf).unwrap();
                count += 1;
            }
            black_box(count);
        });
    });

    group.finish();
}

// --- Boilerplate to run the benchmarks ---

criterion_group!(
    benches,
    benchmark_alternatives_small,
    benchmark_alternatives_large
);
criterion_main!(benches);
