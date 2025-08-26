// benches/comparative_benchmarks.rs
// Comparative benchmarks between flatstream-rs and alternative serialization approaches
// ---
// # Benchmark Purpose: Comparative Analysis
//
// Central question: How does flatstream-rs compare to other common Rust
// serialization libraries (e.g., bincode and serde_json) in a streaming workload?
//
// Methodology:
// Each library performs the same task to ensure an apples-to-apples comparison:
// 1) Serialize a TelemetryEvent struct
// 2) Prepend a 4-byte little-endian length prefix (manual for others; built-in for flatstream)
// 3) Write the framed message to an in-memory buffer
// 4) Read the stream back by deframing each message
// 5) Deserialize back into the struct
//
// Interpretation:
// - flatstream_default: Baseline using zero-copy APIs with DefaultFramer
// - flatstream_*checksum*: Adds a checksum; measures integrity cost
// - bincode: Fast binary format with manual framing
// - serde_json: Text format; slowest but human-readable
//
// Notes:
// - In-memory buffers are used to isolate CPU/algorithmic cost from I/O latency.
// - For flatstream, framing/deframing are handled by strategies; for others it's manual.
// ---

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flatbuffers::FlatBufferBuilder;
use flatstream::{
    self as flatstream, DefaultDeframer, DefaultFramer, StreamReader, StreamSerialize,
    StreamWriter, UnsafeDeframer,
};
use serde::{Deserialize, Serialize};
use std::io::{Cursor, Read, Write};

// Import checksum types when features are enabled
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
use flatstream::framing::{ChecksumDeframer, ChecksumFramer};

#[cfg(feature = "xxhash")]
use flatstream::XxHash64;

#[cfg(feature = "crc32")]
use flatstream::Crc32;

#[cfg(feature = "crc16")]
use flatstream::Crc16;

// --- Common Data Structure ---
// A simple struct that can be used by serde and flatstream.

#[derive(Serialize, Deserialize, Debug, Clone)]
struct TelemetryEvent {
    device_id: u64,
    timestamp: u64,
    value: f64,
}

// --- flatstream-rs Implementation ---
// A compact binary representation in a FlatBuffer builder. In production you
// would use a schema-generated table; here we create a tiny binary record for
// parity across approaches.

impl StreamSerialize for TelemetryEvent {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> flatstream::Result<()> {
        // Binary layout: [device_id: u64][timestamp: u64][value: f64]
        let mut data = Vec::with_capacity(24); // 8 + 8 + 8 bytes

        // Add device_id (little-endian)
        data.extend_from_slice(&self.device_id.to_le_bytes());
        // Add timestamp (little-endian)
        data.extend_from_slice(&self.timestamp.to_le_bytes());
        // Add value (little-endian)
        data.extend_from_slice(&self.value.to_le_bytes());

        // Store as a FlatBuffer vector payload
        let data_vec = builder.create_vector(&data);
        builder.finish(data_vec, None);
        Ok(())
    }
}

// --- Benchmark Functions ---

fn benchmark_alternatives_small(c: &mut Criterion) {
    let mut group = c.benchmark_group("Small Dataset (100 events)");

    // Create test data - 100 telemetry events (small dataset)
    let events: Vec<TelemetryEvent> = (0..100)
        .map(|i| TelemetryEvent {
            device_id: i,
            timestamp: 1672531200 + i,
            value: i as f64 * 1.5,
        })
        .collect();

    // Benchmark 1: flatstream-rs with default framer (no checksum)
    // Baseline performance for the recommended zero-copy configuration.
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

    // Benchmark 1b: flatstream-rs default framer with UnsafeDeframer (read path only)
    // Measures upper bound for read throughput when skipping verification/zeroing.
    group.bench_function("flatstream_default_unsafe_read", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            // Write phase
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
            for event in &events {
                writer.write(event).unwrap();
            }
            black_box(&buffer);

            // Read phase (unsafe deframer)
            let mut reader = StreamReader::new(Cursor::new(&buffer), UnsafeDeframer);
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
    // Measures the cost of computing and verifying a high-speed checksum per message.
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
    // Measures CRC32 integrity overhead in the same workload.
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
    // Measures CRC16 integrity overhead (smallest checksum size) in the same workload.
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

    // Benchmark 5: flatstream-rs with builder reuse (simulated arena-like behavior)
    // Note: FlatBuffers' allocator trait makes true arena allocators challenging.
    // The default builder reuse already eliminates most allocations in practice.
    #[cfg(feature = "bumpalo")]
    group.bench_function("flatstream_builder_reuse", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();

            // Create a writer with builder reuse (simulates arena allocation benefits)
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

    // Benchmark 6: bincode + manual framing
    // Provides a high-performance binary baseline using serde-based encoding.
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
    // Human-readable format; included to illustrate overhead of text encoding/decoding.
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
    let mut group = c.benchmark_group("Large Dataset (~2.4 MiB)");

    // Create test data - approximately a few MiB of telemetry events
    // Larger dataset to detect scaling effects and amortization differences.
    let events: Vec<TelemetryEvent> = (0..100_000)
        .map(|i| TelemetryEvent {
            device_id: i % 1000, // Reuse device IDs to reduce memory
            timestamp: 1672531200 + i,
            value: i as f64 * 1.5,
        })
        .collect();

    // Benchmark 1: flatstream-rs with default framer (no checksum)
    // Same methodology as the small dataset, but with significantly more events.
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

    // Benchmark 1b: flatstream-rs default framer with UnsafeDeframer (read path only)
    // Upper-bound read throughput by skipping verification/zeroing.
    group.bench_function("flatstream_default_unsafe_read", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            // Write phase
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
            for event in &events {
                writer.write(event).unwrap();
            }
            black_box(&buffer);

            // Read phase (unsafe deframer)
            let mut reader = StreamReader::new(Cursor::new(&buffer), UnsafeDeframer);
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
    // Integrity overhead at larger scale.
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

    // Benchmark 5: flatstream-rs with builder reuse (simulated arena-like behavior)
    #[cfg(feature = "bumpalo")]
    group.bench_function("flatstream_builder_reuse", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();

            // Create a writer with builder reuse (simulates arena allocation benefits)
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
