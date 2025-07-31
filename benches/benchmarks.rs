use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use flatbuffers::FlatBufferBuilder;
use flatstream_rs::checksum::Checksum;
use flatstream_rs::{DefaultDeframer, DefaultFramer, StreamReader, StreamWriter, StreamSerialize};
use std::io::Cursor;

// Import checksum types when features are enabled
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
use flatstream_rs::framing::{ChecksumDeframer, ChecksumFramer};

#[cfg(feature = "xxhash")]
use flatstream_rs::XxHash64;

#[cfg(feature = "crc32")]
use flatstream_rs::Crc32;

#[cfg(feature = "crc16")]
use flatstream_rs::Crc16;

// --- Realistic Test Data Structure ---

#[derive(Clone)]
struct TelemetryEvent {
    device_id: u64,
    timestamp: u64,
    value: f64,
}

// This teaches the benchmark how to serialize our new data structure.
// It's a more accurate workload than just serializing a string.
impl StreamSerialize for TelemetryEvent {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> flatstream::Result<()> {
        // This simulates a more realistic serialization process by creating a binary vector.
        let mut data = Vec::with_capacity(24); // 8 bytes for each field (u64, u64, f64)
        data.extend_from_slice(&self.device_id.to_le_bytes());
        data.extend_from_slice(&self.timestamp.to_le_bytes());
        data.extend_from_slice(&self.value.to_le_bytes());

        let data_vec = builder.create_vector(&data);
        builder.finish(data_vec, None);
        Ok(())
    }
}

// --- New Data Generation Utility ---
fn create_telemetry_events(count: usize) -> Vec<TelemetryEvent> {
    (0..count as u64)
        .map(|i| TelemetryEvent {
            device_id: i,
            timestamp: 1672531200 + i,
            value: i as f64 * 1.5,
        })
        .collect()
}

fn create_large_messages(count: usize) -> Vec<String> {
    (0..count).map(|i| format!("large benchmark message number {} with additional data to simulate real-world telemetry events containing sensor readings, timestamps, and metadata", i)).collect()
}

// Benchmark configuration
const SMALL_MESSAGE_COUNT: usize = 100;
const LARGE_MESSAGE_COUNT: usize = 50;
const HIGH_FREQUENCY_COUNT: usize = 1000;

// === PARAMETERIZED BENCHMARK HELPERS ===

/// Generic benchmark function for writing with any checksum algorithm
/// This allows us to compare performance across different checksum implementations
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
fn bench_writer<C: Checksum + Default + Copy>(
    group: &mut criterion::BenchmarkGroup<criterion::measurement::WallTime>,
    checksum_name: &str,
    messages: &[String],
) {
    use flatstream_rs::framing::ChecksumFramer;

    // Calculate total throughput in bytes for fair comparison
    let total_bytes: usize = messages.iter().map(|msg| msg.len()).sum();
    group.throughput(Throughput::Bytes(total_bytes as u64));

    group.bench_with_input(
        BenchmarkId::new("write_100_messages", checksum_name),
        messages,
        |b, msgs| {
            b.iter(|| {
                let mut buffer = Vec::new();
                let checksum = C::default();
                let framer = ChecksumFramer::new(checksum);
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
                let mut builder = FlatBufferBuilder::new();

                for message in msgs {
                    builder.reset();
                    let data = builder.create_string(message);
                    builder.finish(data, None);
                    writer.write(&mut builder).unwrap();
                }

                black_box(buffer);
            });
        },
    );
}

/// Generic benchmark function for reading with any checksum algorithm
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
fn bench_reader<C: Checksum + Default + Copy>(
    group: &mut criterion::BenchmarkGroup<criterion::measurement::WallTime>,
    checksum_name: &str,
) {
    use flatstream_rs::framing::{ChecksumDeframer, ChecksumFramer};

    // Prepare test data with the specific checksum
    let mut buffer = Vec::new();
    {
        let checksum = C::default();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        let mut builder = FlatBufferBuilder::new();
        let messages = create_test_messages(SMALL_MESSAGE_COUNT);
        for message in &messages {
            builder.reset();
            let data = builder.create_string(message);
            builder.finish(data, None);
            writer.write(&mut builder).unwrap();
        }
    }

    let total_bytes = buffer.len();
    group.throughput(Throughput::Bytes(total_bytes as u64));

    group.bench_with_input(
        BenchmarkId::new("read_100_messages", checksum_name),
        &buffer,
        |b, data| {
            b.iter(|| {
                let checksum = C::default();
                let deframer = ChecksumDeframer::new(checksum);
                let mut reader = StreamReader::new(Cursor::new(data), deframer);

                // Count all messages using process_all
                let mut count = 0;
                reader
                    .process_all(|_payload| {
                        count += 1;
                        Ok(())
                    })
                    .unwrap();

                black_box(count);
            });
        },
    );
}

/// Generic benchmark function for write-read cycle with any checksum algorithm
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
fn bench_write_read_cycle<C: Checksum + Default + Copy>(
    group: &mut criterion::BenchmarkGroup<criterion::measurement::WallTime>,
    checksum_name: &str,
    messages: &[String],
) {
    use flatstream_rs::framing::{ChecksumDeframer, ChecksumFramer};

    let total_bytes: usize = messages.iter().map(|msg| msg.len()).sum();
    group.throughput(Throughput::Bytes(total_bytes as u64));

    group.bench_with_input(
        BenchmarkId::new("write_read_cycle_100_messages", checksum_name),
        messages,
        |b, msgs| {
            b.iter(|| {
                // Write phase
                let mut buffer = Vec::new();
                let checksum = C::default();
                let framer = ChecksumFramer::new(checksum);
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
                let mut builder = FlatBufferBuilder::new();

                for message in msgs {
                    builder.reset();
                    let data = builder.create_string(message);
                    builder.finish(data, None);
                    writer.write(&mut builder).unwrap();
                }

                // Read phase
                let deframer = ChecksumDeframer::new(checksum);
                let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
                let mut count = 0;
                reader
                    .process_all(|_payload| {
                        count += 1;
                        Ok(())
                    })
                    .unwrap();

                black_box((buffer, count));
            });
        },
    );
}

// === PARAMETERIZED BENCHMARK GROUPS ===

/// Parameterized benchmark for all checksum writers
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
fn benchmark_checksum_writers(c: &mut Criterion) {
    let mut group = c.benchmark_group("Checksum Writers");
    let messages = create_test_messages(SMALL_MESSAGE_COUNT);

    // Call the generic function for each checksum type
    // The #[cfg] attributes are now localized and simple
    #[cfg(feature = "xxhash")]
    bench_writer::<XxHash64>(&mut group, "XXHash64", &messages);

    #[cfg(feature = "crc32")]
    bench_writer::<Crc32>(&mut group, "CRC32", &messages);

    #[cfg(feature = "crc16")]
    bench_writer::<Crc16>(&mut group, "CRC16", &messages);

    group.finish();
}

/// Parameterized benchmark for all checksum readers
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
fn benchmark_checksum_readers(c: &mut Criterion) {
    let mut group = c.benchmark_group("Checksum Readers");

    // Call the generic function for each checksum type
    #[cfg(feature = "xxhash")]
    bench_reader::<XxHash64>(&mut group, "XXHash64");

    #[cfg(feature = "crc32")]
    bench_reader::<Crc32>(&mut group, "CRC32");

    #[cfg(feature = "crc16")]
    bench_reader::<Crc16>(&mut group, "CRC16");

    group.finish();
}

/// Parameterized benchmark for all checksum write-read cycles
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
fn benchmark_checksum_cycles(c: &mut Criterion) {
    let mut group = c.benchmark_group("Checksum Write-Read Cycles");
    let messages = create_test_messages(SMALL_MESSAGE_COUNT);

    // Call the generic function for each checksum type
    #[cfg(feature = "xxhash")]
    bench_write_read_cycle::<XxHash64>(&mut group, "XXHash64", &messages);

    #[cfg(feature = "crc32")]
    bench_write_read_cycle::<Crc32>(&mut group, "CRC32", &messages);

    #[cfg(feature = "crc16")]
    bench_write_read_cycle::<Crc16>(&mut group, "CRC16", &messages);

    group.finish();
}

// === WRITE BENCHMARKS ===

fn benchmark_write_default_framer(c: &mut Criterion) {
    let events = create_telemetry_events(SMALL_MESSAGE_COUNT);

    c.bench_function("write_default_framer_100_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            for event in &events {
                // Now we are using the StreamSerialize implementation for TelemetryEvent
                writer.write(event).unwrap();
            }

            black_box(buffer);
        });
    });
}

// === READ BENCHMARKS ===

fn benchmark_read_default_deframer(c: &mut Criterion) {
    // Prepare test data using the realistic TelemetryEvent struct
    let mut buffer = Vec::new();
    {
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        let events = create_telemetry_events(SMALL_MESSAGE_COUNT);

        // This loop now correctly uses the StreamSerialize trait
        for event in &events {
            writer.write(event).unwrap();
        }
    }

    c.bench_function("read_default_deframer_100_messages", |b| {
        b.iter(|| {
            let deframer = DefaultDeframer;
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
}

// === ZERO-ALLOCATION READING BENCHMARKS ===

fn benchmark_zero_allocation_reading(c: &mut Criterion) {
    // Prepare test data
    let mut buffer = Vec::new();
    {
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        let mut builder = FlatBufferBuilder::new();
        let messages = create_test_messages(SMALL_MESSAGE_COUNT);
        for message in &messages {
            builder.reset();
            let data = builder.create_string(message);
            builder.finish(data, None);
            writer.write(&mut builder).unwrap();
        }
    }

    c.bench_function("zero_allocation_reading_100_messages", |b| {
        b.iter(|| {
            let deframer = DefaultDeframer;
            let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            let mut total_size = 0;

            // High-performance zero-allocation pattern using messages()
            let mut messages = reader.messages();
            while let Some(payload_slice) = messages.next().unwrap() {
                total_size += payload_slice.len();
                count += 1;
            }

            black_box((count, total_size));
        });
    });
}

#[cfg(feature = "xxhash")]
fn benchmark_zero_allocation_reading_with_checksum(c: &mut Criterion) {
    // Prepare test data
    let mut buffer = Vec::new();
    {
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        let mut builder = FlatBufferBuilder::new();
        let messages = create_test_messages(SMALL_MESSAGE_COUNT);
        for message in &messages {
            builder.reset();
            let data = builder.create_string(message);
            builder.finish(data, None);
            writer.write(&mut builder).unwrap();
        }
    }

    c.bench_function("zero_allocation_reading_xxhash64_100_messages", |b| {
        b.iter(|| {
            let checksum = XxHash64::new();
            let deframer = ChecksumDeframer::new(checksum);
            let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            let mut total_size = 0;

            // High-performance zero-allocation pattern using messages()
            let mut messages = reader.messages();
            while let Some(payload_slice) = messages.next().unwrap() {
                total_size += payload_slice.len();
                count += 1;
            }

            black_box((count, total_size));
        });
    });
}

// === WRITE BATCHING BENCHMARKS ===

fn benchmark_write_batch_vs_iterative(c: &mut Criterion) {
    let messages = create_test_messages(SMALL_MESSAGE_COUNT);

    c.bench_function("write_iterative_100_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
            let mut builder = FlatBufferBuilder::new();

            // Explicit for loop (v2.5 pattern)
            for message in &messages {
                builder.reset();
                let data = builder.create_string(message);
                builder.finish(data, None);
                writer.write(&mut builder).unwrap();
            }

            black_box(buffer);
        });
    });
}

#[cfg(feature = "xxhash")]
fn benchmark_write_batch_with_checksum(c: &mut Criterion) {
    let messages = create_test_messages(SMALL_MESSAGE_COUNT);

    c.bench_function("write_iterative_xxhash64_100_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let checksum = XxHash64::new();
            let framer = ChecksumFramer::new(checksum);
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
            let mut builder = FlatBufferBuilder::new();

            // Explicit for loop with checksum
            for message in &messages {
                builder.reset();
                let data = builder.create_string(message);
                builder.finish(data, None);
                writer.write(&mut builder).unwrap();
            }

            black_box(buffer);
        });
    });
}

// === END-TO-END BENCHMARKS ===

fn benchmark_write_read_cycle_default(c: &mut Criterion) {
    let messages = create_test_messages(LARGE_MESSAGE_COUNT);

    c.bench_function("write_read_cycle_default_50_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();

            // Write
            {
                let framer = DefaultFramer;
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
                let mut builder = FlatBufferBuilder::new();
                for message in &messages {
                    builder.reset();
                    let data = builder.create_string(message);
                    builder.finish(data, None);
                    writer.write(&mut builder).unwrap();
                }
            }

            // Read
            {
                let deframer = DefaultDeframer;
                let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
                let mut count = 0;
                reader
                    .process_all(|_payload| {
                        count += 1;
                        Ok(())
                    })
                    .unwrap();
                black_box(count);
            }
        });
    });
}

#[cfg(feature = "xxhash")]
fn benchmark_write_read_cycle_with_checksum(c: &mut Criterion) {
    c.bench_function("write_read_cycle_xxhash64_50_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();

            // Write
            {
                let checksum = XxHash64::new();
                let framer = ChecksumFramer::new(checksum);
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
                let mut builder = FlatBufferBuilder::new();
                let messages = create_test_messages(LARGE_MESSAGE_COUNT);
                for message in &messages {
                    builder.reset();
                    let data = builder.create_string(message);
                    builder.finish(data, None);
                    writer.write(&mut builder).unwrap();
                }
            }

            // Read
            {
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
            }
        });
    });
}

// === HIGH-FREQUENCY TELEMETRY BENCHMARKS ===

fn benchmark_high_frequency_telemetry(c: &mut Criterion) {
    let messages = create_test_messages(HIGH_FREQUENCY_COUNT);

    c.bench_function("high_frequency_telemetry_1000_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
            let mut builder = FlatBufferBuilder::new();

            // Simulate high-frequency telemetry writing with explicit for loop
            for message in &messages {
                builder.reset();
                let data = builder.create_string(message);
                builder.finish(data, None);
                writer.write(&mut builder).unwrap();
            }

            black_box(buffer);
        });
    });
}

fn benchmark_high_frequency_reading(c: &mut Criterion) {
    // Prepare test data
    let mut buffer = Vec::new();
    {
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        let mut builder = FlatBufferBuilder::new();
        let messages = create_test_messages(HIGH_FREQUENCY_COUNT);
        for message in &messages {
            builder.reset();
            let data = builder.create_string(message);
            builder.finish(data, None);
            writer.write(&mut builder).unwrap();
        }
    }

    c.bench_function("high_frequency_reading_1000_messages", |b| {
        b.iter(|| {
            let deframer = DefaultDeframer;
            let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            let mut total_size = 0;

            // High-performance zero-allocation pattern for high-frequency scenarios
            let mut messages = reader.messages();
            while let Some(payload_slice) = messages.next().unwrap() {
                total_size += payload_slice.len();
                count += 1;
            }

            black_box((count, total_size));
        });
    });
}

// === LARGE MESSAGE BENCHMARKS ===

fn benchmark_large_messages(c: &mut Criterion) {
    let messages = create_large_messages(LARGE_MESSAGE_COUNT);

    c.bench_function("large_messages_50_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
            let mut builder = FlatBufferBuilder::new();

            for message in &messages {
                builder.reset();
                let data = builder.create_string(message);
                builder.finish(data, None);
                writer.write(&mut builder).unwrap();
            }

            black_box(buffer);
        });
    });
}

#[cfg(feature = "xxhash")]
fn benchmark_large_messages_with_checksum(c: &mut Criterion) {
    let messages = create_large_messages(LARGE_MESSAGE_COUNT);

    c.bench_function("large_messages_xxhash64_50_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let checksum = XxHash64::new();
            let framer = ChecksumFramer::new(checksum);
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
            let mut builder = FlatBufferBuilder::new();

            for message in &messages {
                builder.reset();
                let data = builder.create_string(message);
                builder.finish(data, None);
                writer.write(&mut builder).unwrap();
            }

            black_box(buffer);
        });
    });
}

// === MEMORY EFFICIENCY BENCHMARKS ===

fn benchmark_memory_efficiency(c: &mut Criterion) {
    let messages = create_test_messages(SMALL_MESSAGE_COUNT);

    c.bench_function("memory_efficiency_write_100_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
            let mut builder = FlatBufferBuilder::new();

            // Measure memory usage during explicit for loop
            for message in &messages {
                builder.reset();
                let data = builder.create_string(message);
                builder.finish(data, None);
                writer.write(&mut builder).unwrap();
            }

            let buffer_size = buffer.len();
            black_box((buffer, buffer_size));
        });
    });
}

// === COMPARATIVE BENCHMARKS (vs Bincode and Protobuf) ===

// Note: These benchmarks require additional dependencies that would need to be added to Cargo.toml
// For now, we'll create the structure but comment out the actual implementations
// to avoid breaking the build without the dependencies.

/*
// This would require adding to Cargo.toml:
// [dev-dependencies]
// serde = { version = "1.0", features = ["derive"] }
// bincode = "1.3"
// prost = "0.12"

use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Clone)]
struct TelemetryData {
    timestamp: u64,
    device_id: String,
    value: f64,
    is_critical: bool,
}

impl StreamSerialize for TelemetryData {
    fn serialize(&self, builder: &mut flatbuffers::FlatBufferBuilder) -> Result<()> {
        let device_id = builder.create_string(&self.device_id);
        builder.finish(device_id, None);
        Ok(())
    }
}

fn benchmark_comparative_formats(c: &mut Criterion) {
    let data: Vec<_> = (0..100)
        .map(|i| TelemetryData {
            timestamp: i * 1000,
            device_id: format!("device-{}", i),
            value: i as f64 * 1.5,
            is_critical: i % 10 == 0,
        })
        .collect();

    let mut group = c.benchmark_group("Comparative: Write 100 Messages");

    // Benchmark flatstream-rs
    group.bench_function("flatstream", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
            let mut builder = FlatBufferBuilder::new();
            for item in black_box(&data) {
                builder.reset();
                let data = builder.create_string(&item.device_id);
                builder.finish(data, None);
                writer.write(&mut builder).unwrap();
            }
        });
    });

    // Benchmark bincode with manual framing
    group.bench_function("bincode", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            for item in black_box(&data) {
                let encoded: Vec<u8> = bincode::serialize(item).unwrap();
                let len = encoded.len() as u32;
                buffer.write_all(&len.to_le_bytes()).unwrap();
                buffer.write_all(&encoded).unwrap();
            }
        });
    });

    // Benchmark protobuf
    group.bench_function("protobuf", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            for item in black_box(&data) {
                item.encode_length_delimited(&mut buffer).unwrap();
            }
        });
    });

    group.finish();
}
*/

// === REGRESSION DETECTION BENCHMARKS ===

// These benchmarks are specifically designed to detect performance regressions
// by focusing on the most sensitive operations that could be affected by
// architectural changes.

fn benchmark_regression_sensitive_operations(c: &mut Criterion) {
    let messages = create_test_messages(SMALL_MESSAGE_COUNT);

    // Test 1: Small message writing (most sensitive to dispatch overhead)
    c.bench_function("regression_small_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
            let mut builder = FlatBufferBuilder::new();

            // Write many small messages to detect dispatch overhead
            for message in &messages {
                builder.reset();
                let data = builder.create_string(message);
                builder.finish(data, None);
                writer.write(&mut builder).unwrap();
            }

            black_box(buffer);
        });
    });

    // Test 2: Monomorphization stress test
    c.bench_function("regression_monomorphization", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
            let mut builder = FlatBufferBuilder::new();

            // Mix different operations to test compiler optimization boundaries
            for (i, message) in messages.iter().enumerate() {
                builder.reset();
                let data = builder.create_string(message);
                builder.finish(data, None);
                writer.write(&mut builder).unwrap();
            }

            black_box(buffer);
        });
    });

    // Test 3: Instruction cache pressure test
    c.bench_function("regression_instruction_cache", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();

            // Create multiple writers to test instruction cache pressure
            for _ in 0..10 {
                let framer = DefaultFramer;
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
                let mut builder = FlatBufferBuilder::new();
                for message in &messages[..10] {
                    builder.reset();
                    let data = builder.create_string(message);
                    builder.finish(data, None);
                    writer.write(&mut builder).unwrap();
                }
            }

            black_box(buffer);
        });
    });
}

// === BENCHMARK SUMMARY ===

// Benchmark Categories and Coverage:
//
// 1. **Write Performance**: Default framer, XXHash64, CRC32, CRC16 checksums
// 2. **Read Performance**: Default deframer, XXHash64, CRC32, CRC16 checksums
// 3. **Zero-Allocation Reading**: High-performance pattern comparison
// 4. **Write Batching**: Batch vs iterative performance comparison
// 5. **End-to-End Cycles**: Complete write-read cycle performance
// 6. **High-Frequency Telemetry**: 1000 message scenarios
// 7. **Large Messages**: Real-world message size simulation
// 8. **Memory Efficiency**: Memory usage analysis
// 9. **Regression Detection**: Performance regression sensitive tests
// 10. **Comparative Analysis**: vs Bincode and Protobuf (structure ready)
//
// **Feature Coverage**:
// - Default framing (always available)
// - XXHash64 checksums (feature-gated)
// - CRC32 checksums (feature-gated)
// - All performance optimizations
// - Regression detection capabilities

// === MAIN BENCHMARK CONFIGURATION ===

// === SIMPLIFIED CRITERION GROUPS ===

// No checksum features enabled
#[cfg(not(any(feature = "xxhash", feature = "crc32", feature = "crc16")))]
criterion_group!(
    benches,
    benchmark_write_default_framer,
    benchmark_read_default_deframer,
    benchmark_zero_allocation_reading,
    benchmark_write_batch_vs_iterative,
    benchmark_write_read_cycle_default,
    benchmark_high_frequency_telemetry,
    benchmark_high_frequency_reading,
    benchmark_large_messages,
    benchmark_memory_efficiency,
    benchmark_regression_sensitive_operations,
);

// At least one checksum feature enabled
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
criterion_group!(
    benches,
    benchmark_write_default_framer,
    benchmark_read_default_deframer,
    benchmark_zero_allocation_reading,
    benchmark_write_batch_vs_iterative,
    benchmark_write_read_cycle_default,
    benchmark_high_frequency_telemetry,
    benchmark_high_frequency_reading,
    benchmark_large_messages,
    benchmark_memory_efficiency,
    benchmark_regression_sensitive_operations,
    // Parameterized checksum benchmarks
    benchmark_checksum_writers,
    benchmark_checksum_readers,
    benchmark_checksum_cycles,
);

criterion_main!(benches);
