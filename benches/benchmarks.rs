use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use flatbuffers::FlatBufferBuilder;
use flatstream::checksum::Checksum;
use flatstream::{
    DefaultDeframer, DefaultFramer, SafeTakeDeframer, StreamReader, StreamSerialize, StreamWriter,
    UnsafeDeframer,
};
use std::io::Cursor;

// Import checksum types when features are enabled
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
use flatstream::framing::{ChecksumDeframer, ChecksumFramer};

#[cfg(feature = "xxhash")]
use flatstream::XxHash64;

#[cfg(feature = "crc32")]
use flatstream::Crc32;

#[cfg(feature = "crc16")]
use flatstream::Crc16;

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
    events: &[TelemetryEvent],
) {
    use flatstream::framing::ChecksumFramer;

    // Calculate total throughput in bytes for fair comparison
    let total_bytes: usize = events.len() * 24; // Each TelemetryEvent is 24 bytes
    group.throughput(Throughput::Bytes(total_bytes as u64));

    group.bench_with_input(
        BenchmarkId::new("write_100_messages", checksum_name),
        events,
        |b, evts| {
            b.iter(|| {
                let mut buffer = Vec::new();
                let checksum = C::default();
                let framer = ChecksumFramer::new(checksum);
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

                for event in evts {
                    writer.write(event).unwrap();
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
    use flatstream::framing::{ChecksumDeframer, ChecksumFramer};

    // Prepare test data with the specific checksum
    let mut buffer = Vec::new();
    {
        let checksum = C::default();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        let events = create_telemetry_events(SMALL_MESSAGE_COUNT);
        for event in &events {
            writer.write(event).unwrap();
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
    events: &[TelemetryEvent],
) {
    use flatstream::framing::{ChecksumDeframer, ChecksumFramer};

    let total_bytes: usize = events.len() * 24; // Each TelemetryEvent is 24 bytes
    group.throughput(Throughput::Bytes(total_bytes as u64));

    group.bench_with_input(
        BenchmarkId::new("write_read_cycle_100_messages", checksum_name),
        events,
        |b, evts| {
            b.iter(|| {
                // Write phase
                let mut buffer = Vec::new();
                let checksum = C::default();
                let framer = ChecksumFramer::new(checksum);
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

                for event in evts {
                    writer.write(event).unwrap();
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
    let events = create_telemetry_events(SMALL_MESSAGE_COUNT);

    // Call the generic function for each checksum type
    // The #[cfg] attributes are now localized and simple
    #[cfg(feature = "xxhash")]
    bench_writer::<XxHash64>(&mut group, "XXHash64", &events);

    #[cfg(feature = "crc32")]
    bench_writer::<Crc32>(&mut group, "CRC32", &events);

    #[cfg(feature = "crc16")]
    bench_writer::<Crc16>(&mut group, "CRC16", &events);

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
    let events = create_telemetry_events(SMALL_MESSAGE_COUNT);

    // Call the generic function for each checksum type
    #[cfg(feature = "xxhash")]
    bench_write_read_cycle::<XxHash64>(&mut group, "XXHash64", &events);

    #[cfg(feature = "crc32")]
    bench_write_read_cycle::<Crc32>(&mut group, "CRC32", &events);

    #[cfg(feature = "crc16")]
    bench_write_read_cycle::<Crc16>(&mut group, "CRC16", &events);

    group.finish();
}

// === WRITE BENCHMARKS ===

fn benchmark_write_default_framer(c: &mut Criterion) {
    let events = create_telemetry_events(SMALL_MESSAGE_COUNT);

    c.bench_function("write_default_framer_100_messages", |b| {
        // The builder is now created ONCE, outside the hot loop.
        let mut builder = FlatBufferBuilder::new();

        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            for event in &events {
                // This is the realistic high-performance pattern:
                // 1. Reset the builder (reuses its memory).
                // 2. Serialize the new data.
                // 3. Write the finished buffer.
                builder.reset();
                event.serialize(&mut builder).unwrap();
                writer.write_finished(&mut builder).unwrap();
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
    // Prepare test data using realistic TelemetryEvent data
    let mut buffer = Vec::new();
    {
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        let events = create_telemetry_events(SMALL_MESSAGE_COUNT);
        for event in &events {
            writer.write(event).unwrap();
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
    // Prepare test data using realistic TelemetryEvent data
    let mut buffer = Vec::new();
    {
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        let events = create_telemetry_events(SMALL_MESSAGE_COUNT);
        for event in &events {
            writer.write(event).unwrap();
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
    let events = create_telemetry_events(SMALL_MESSAGE_COUNT);

    c.bench_function("write_iterative_100_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            // Explicit for loop (v2.5 pattern) with realistic data
            for event in &events {
                writer.write(event).unwrap();
            }

            black_box(buffer);
        });
    });
}

#[cfg(feature = "xxhash")]
fn benchmark_write_batch_with_checksum(c: &mut Criterion) {
    let events = create_telemetry_events(SMALL_MESSAGE_COUNT);

    c.bench_function("write_iterative_xxhash64_100_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let checksum = XxHash64::new();
            let framer = ChecksumFramer::new(checksum);
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            // Explicit for loop with checksum and realistic data
            for event in &events {
                writer.write(event).unwrap();
            }

            black_box(buffer);
        });
    });
}

// === END-TO-END BENCHMARKS ===

fn benchmark_write_read_cycle_default(c: &mut Criterion) {
    let events = create_telemetry_events(LARGE_MESSAGE_COUNT);

    c.bench_function("write_read_cycle_default_50_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();

            // Write
            {
                let framer = DefaultFramer;
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
                for event in &events {
                    writer.write(event).unwrap();
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
                let events = create_telemetry_events(LARGE_MESSAGE_COUNT);
                for event in &events {
                    writer.write(event).unwrap();
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
    let events = create_telemetry_events(HIGH_FREQUENCY_COUNT);

    c.bench_function("high_frequency_telemetry_1000_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            // Simulate high-frequency telemetry writing with explicit for loop
            for event in &events {
                writer.write(event).unwrap();
            }

            black_box(buffer);
        });
    });
}

fn benchmark_high_frequency_reading(c: &mut Criterion) {
    // Prepare test data using realistic TelemetryEvent data
    let mut buffer = Vec::new();
    {
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        let events = create_telemetry_events(HIGH_FREQUENCY_COUNT);
        for event in &events {
            writer.write(event).unwrap();
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
    let events = create_telemetry_events(LARGE_MESSAGE_COUNT);

    c.bench_function("large_messages_50_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            for event in &events {
                writer.write(event).unwrap();
            }

            black_box(buffer);
        });
    });
}

#[cfg(feature = "xxhash")]
fn benchmark_large_messages_with_checksum(c: &mut Criterion) {
    let events = create_telemetry_events(LARGE_MESSAGE_COUNT);

    c.bench_function("large_messages_xxhash64_50_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let checksum = XxHash64::new();
            let framer = ChecksumFramer::new(checksum);
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            for event in &events {
                writer.write(event).unwrap();
            }

            black_box(buffer);
        });
    });
}

// === MEMORY EFFICIENCY BENCHMARKS ===

fn benchmark_memory_efficiency(c: &mut Criterion) {
    let events = create_telemetry_events(SMALL_MESSAGE_COUNT);

    c.bench_function("memory_efficiency_write_100_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            // Measure memory usage during explicit for loop with realistic data
            for event in &events {
                writer.write(event).unwrap();
            }

            let buffer_size = buffer.len();
            black_box((buffer, buffer_size));
        });
    });
}

// === REGRESSION DETECTION BENCHMARKS ===

fn benchmark_regression_sensitive_operations(c: &mut Criterion) {
    let events = create_telemetry_events(SMALL_MESSAGE_COUNT);

    // Test 1: Small message writing (most sensitive to dispatch overhead)
    c.bench_function("regression_small_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            // Write many small messages to detect dispatch overhead
            for event in &events {
                writer.write(event).unwrap();
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

            // Mix different operations to test compiler optimization boundaries
            for event in &events {
                writer.write(event).unwrap();
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
                for event in &events[..10] {
                    writer.write(event).unwrap();
                }
            }

            black_box(buffer);
        });
    });
}

// === READ PATH ALTERNATIVES BENCHMARKS ===

// In: benches/benchmarks.rs

fn benchmark_read_path_alternatives(c: &mut Criterion) {
    // 1. Prepare a consistent set of test data
    let mut buffer = Vec::new();
    {
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(std::io::Cursor::new(&mut buffer), framer);
        for i in 0..100 {
            let msg = format!("message {}", i);
            writer.write(&msg).unwrap();
        }
    }

    let mut group = c.benchmark_group("Read Path Implementations");

    // ADD THIS BLOCK TO YOUR FUNCTION
    // --- Benchmark the original DefaultDeframer as a baseline ---
    group.bench_function("DefaultDeframer (Original)", |b| {
        b.iter(|| {
            let deframer = DefaultDeframer;
            let mut reader = StreamReader::new(std::io::Cursor::new(&buffer), deframer);
            reader
                .process_all(|payload| {
                    black_box(payload);
                    Ok(())
                })
                .unwrap();
        });
    });

    // 2. Benchmark the safe `Read::take` implementation
    group.bench_function("SafeTakeDeframer", |b| {
        b.iter(|| {
            let deframer = SafeTakeDeframer;
            let mut reader = StreamReader::new(std::io::Cursor::new(&buffer), deframer);
            reader
                .process_all(|payload| {
                    black_box(payload);
                    Ok(())
                })
                .unwrap();
        });
    });

    // 3. Benchmark the `unsafe` implementation
    group.bench_function("UnsafeDeframer", |b| {
        b.iter(|| {
            let deframer = UnsafeDeframer;
            let mut reader = StreamReader::new(std::io::Cursor::new(&buffer), deframer);
            reader
                .process_all(|payload| {
                    black_box(payload);
                    Ok(())
                })
                .unwrap();
        });
    });

    group.finish();
}

// === MAIN BENCHMARK CONFIGURATION ===

// Group for benchmarks that run WITHOUT any checksum features
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
    benchmark_read_path_alternatives,
);

// Group for benchmarks that run WITH any checksum feature
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

// Group for benchmarks that are SPECIFIC to the xxhash feature
#[cfg(feature = "xxhash")]
criterion_group!(
    name = xxhash_specific_benches;
    config = Criterion::default();
    targets =
        benchmark_zero_allocation_reading_with_checksum,
        benchmark_write_batch_with_checksum,
        benchmark_write_read_cycle_with_checksum,
        benchmark_large_messages_with_checksum
);

// === MAIN MACRO ===

// Conditionally compile the main macro based on features
#[cfg(all(
    not(feature = "xxhash"),
    not(feature = "crc32"),
    not(feature = "crc16")
))]
criterion_main!(benches);

#[cfg(all(feature = "xxhash", not(feature = "crc32"), not(feature = "crc16")))]
criterion_main!(benches, xxhash_specific_benches);

// Add more combinations if needed for crc32, crc16, etc.
// For simplicity, this handles the two main cases: no checksums, or xxhash is present.
// A more robust solution would handle all 2^3 combinations.

// A simpler catch-all for when any checksum is enabled but we only have xxhash specific benches
#[cfg(all(
    any(feature = "xxhash", feature = "crc32", feature = "crc16"),
    not(all(not(feature = "xxhash")))
))]
criterion_main!(benches, xxhash_specific_benches);

#[cfg(all(
    any(feature = "xxhash", feature = "crc32", feature = "crc16"),
    all(not(feature = "xxhash"))
))]
criterion_main!(benches);
