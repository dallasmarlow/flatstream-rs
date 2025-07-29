use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flatstream_rs::{DefaultDeframer, DefaultFramer, StreamReader, StreamWriter};
use std::io::Cursor;

// Import checksum types when features are enabled
#[cfg(feature = "xxhash")]
use flatstream_rs::{ChecksumDeframer, ChecksumFramer, XxHash64};

#[cfg(feature = "crc32")]
use flatstream_rs::Crc32;

// Test data generation utilities
fn create_test_messages(count: usize) -> Vec<String> {
    (0..count).map(|i| format!("benchmark message number {}", i)).collect()
}

fn create_large_messages(count: usize) -> Vec<String> {
    (0..count).map(|i| format!("large benchmark message number {} with additional data to simulate real-world telemetry events containing sensor readings, timestamps, and metadata", i)).collect()
}

// Benchmark configuration
const SMALL_MESSAGE_COUNT: usize = 100;
const LARGE_MESSAGE_COUNT: usize = 50;
const HIGH_FREQUENCY_COUNT: usize = 1000;

// === WRITE BENCHMARKS ===

fn benchmark_write_default_framer(c: &mut Criterion) {
    let messages = create_test_messages(SMALL_MESSAGE_COUNT);
    
    c.bench_function("write_default_framer_100_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            for message in &messages {
                writer.write(message).unwrap();
            }

            black_box(buffer);
        });
    });
}

#[cfg(feature = "xxhash")]
fn benchmark_write_xxhash64_checksum(c: &mut Criterion) {
    let messages = create_test_messages(SMALL_MESSAGE_COUNT);
    
    c.bench_function("write_xxhash64_checksum_100_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let checksum = XxHash64::new();
            let framer = ChecksumFramer::new(checksum);
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            for message in &messages {
                writer.write(message).unwrap();
            }

            black_box(buffer);
        });
    });
}

#[cfg(feature = "crc32")]
fn benchmark_write_crc32_checksum(c: &mut Criterion) {
    let messages = create_test_messages(SMALL_MESSAGE_COUNT);
    
    c.bench_function("write_crc32_checksum_100_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let checksum = Crc32::new();
            let framer = ChecksumFramer::new(checksum);
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            for message in &messages {
                writer.write(message).unwrap();
            }

            black_box(buffer);
        });
    });
}

// === READ BENCHMARKS ===

fn benchmark_read_default_deframer(c: &mut Criterion) {
    // Prepare test data
    let mut buffer = Vec::new();
    {
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        let messages = create_test_messages(SMALL_MESSAGE_COUNT);
        for message in &messages {
            writer.write(message).unwrap();
        }
    }

    c.bench_function("read_default_deframer_100_messages", |b| {
        b.iter(|| {
            let deframer = DefaultDeframer;
            let reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            for result in reader {
                black_box(result.unwrap());
                count += 1;
            }
            black_box(count);
        });
    });
}

#[cfg(feature = "xxhash")]
fn benchmark_read_xxhash64_checksum(c: &mut Criterion) {
    // Prepare test data
    let mut buffer = Vec::new();
    {
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        let messages = create_test_messages(SMALL_MESSAGE_COUNT);
        for message in &messages {
            writer.write(message).unwrap();
        }
    }

    c.bench_function("read_xxhash64_checksum_100_messages", |b| {
        b.iter(|| {
            let checksum = XxHash64::new();
            let deframer = ChecksumDeframer::new(checksum);
            let reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            for result in reader {
                black_box(result.unwrap());
                count += 1;
            }
            black_box(count);
        });
    });
}

#[cfg(feature = "crc32")]
fn benchmark_read_crc32_checksum(c: &mut Criterion) {
    // Prepare test data
    let mut buffer = Vec::new();
    {
        let checksum = Crc32::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        let messages = create_test_messages(SMALL_MESSAGE_COUNT);
        for message in &messages {
            writer.write(message).unwrap();
        }
    }

    c.bench_function("read_crc32_checksum_100_messages", |b| {
        b.iter(|| {
            let checksum = Crc32::new();
            let deframer = ChecksumDeframer::new(checksum);
            let reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            for result in reader {
                black_box(result.unwrap());
                count += 1;
            }
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
        let messages = create_test_messages(SMALL_MESSAGE_COUNT);
        for message in &messages {
            writer.write(message).unwrap();
        }
    }

    c.bench_function("zero_allocation_reading_100_messages", |b| {
        b.iter(|| {
            let deframer = DefaultDeframer;
            let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            let mut total_size = 0;
            
            // High-performance zero-allocation pattern
            while let Some(payload_slice) = reader.read_message().unwrap() {
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
        let messages = create_test_messages(SMALL_MESSAGE_COUNT);
        for message in &messages {
            writer.write(message).unwrap();
        }
    }

    c.bench_function("zero_allocation_reading_xxhash64_100_messages", |b| {
        b.iter(|| {
            let checksum = XxHash64::new();
            let deframer = ChecksumDeframer::new(checksum);
            let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            let mut total_size = 0;
            
            // High-performance zero-allocation pattern
            while let Some(payload_slice) = reader.read_message().unwrap() {
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

    c.bench_function("write_batch_100_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            // Call the new batch method
            writer.write_batch(&messages).unwrap();

            black_box(buffer);
        });
    });

    c.bench_function("write_iterative_100_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            // Call the old iterative method
            for message in &messages {
                writer.write(message).unwrap();
            }

            black_box(buffer);
        });
    });
}

#[cfg(feature = "xxhash")]
fn benchmark_write_batch_with_checksum(c: &mut Criterion) {
    let messages = create_test_messages(SMALL_MESSAGE_COUNT);

    c.bench_function("write_batch_xxhash64_100_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let checksum = XxHash64::new();
            let framer = ChecksumFramer::new(checksum);
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            writer.write_batch(&messages).unwrap();

            black_box(buffer);
        });
    });
}

// === END-TO-END BENCHMARKS ===

fn benchmark_write_read_cycle_default(c: &mut Criterion) {
    c.bench_function("write_read_cycle_default_50_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();

            // Write
            {
                let framer = DefaultFramer;
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
                let messages = create_test_messages(LARGE_MESSAGE_COUNT);
                for message in &messages {
                    writer.write(message).unwrap();
                }
            }

            // Read
            {
                let deframer = DefaultDeframer;
                let reader = StreamReader::new(Cursor::new(&buffer), deframer);
                let mut count = 0;
                for result in reader {
                    black_box(result.unwrap());
                    count += 1;
                }
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
                let messages = create_test_messages(LARGE_MESSAGE_COUNT);
                for message in &messages {
                    writer.write(message).unwrap();
                }
            }

            // Read
            {
                let checksum = XxHash64::new();
                let deframer = ChecksumDeframer::new(checksum);
                let reader = StreamReader::new(Cursor::new(&buffer), deframer);
                let mut count = 0;
                for result in reader {
                    black_box(result.unwrap());
                    count += 1;
                }
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

            // Simulate high-frequency telemetry writing
            writer.write_batch(&messages).unwrap();

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
        let messages = create_test_messages(HIGH_FREQUENCY_COUNT);
        writer.write_batch(&messages).unwrap();
    }

    c.bench_function("high_frequency_reading_1000_messages", |b| {
        b.iter(|| {
            let deframer = DefaultDeframer;
            let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            let mut total_size = 0;
            
            // High-performance zero-allocation pattern for high-frequency scenarios
            while let Some(payload_slice) = reader.read_message().unwrap() {
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

            for message in &messages {
                writer.write(message).unwrap();
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

            for message in &messages {
                writer.write(message).unwrap();
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

            // Measure memory usage during batch write
            writer.write_batch(&messages).unwrap();
            
            let buffer_size = buffer.len();
            black_box((buffer, buffer_size));
        });
    });
}

// === BENCHMARK SUMMARY ===

// Benchmark Categories and Coverage:
// 
// 1. **Write Performance**: Default framer, XXHash64, CRC32 checksums
// 2. **Read Performance**: Default deframer, XXHash64, CRC32 checksums  
// 3. **Zero-Allocation Reading**: High-performance pattern comparison
// 4. **Write Batching**: Batch vs iterative performance comparison
// 5. **End-to-End Cycles**: Complete write-read cycle performance
// 6. **High-Frequency Telemetry**: 1000 message scenarios
// 7. **Large Messages**: Real-world message size simulation
// 8. **Memory Efficiency**: Memory usage analysis
// 
// **Feature Coverage**:
// - Default framing (always available)
// - XXHash64 checksums (feature-gated)
// - CRC32 checksums (feature-gated)
// - All performance optimizations

// === MAIN BENCHMARK CONFIGURATION ===

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
    // Feature-gated benchmarks
    benchmark_write_xxhash64_checksum,
    benchmark_read_xxhash64_checksum,
    benchmark_zero_allocation_reading_with_checksum,
    benchmark_write_batch_with_checksum,
    benchmark_write_read_cycle_with_checksum,
    benchmark_large_messages_with_checksum,
    benchmark_write_crc32_checksum,
    benchmark_read_crc32_checksum,
);

criterion_main!(benches);
