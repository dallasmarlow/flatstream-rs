//! Example demonstrating sized checksums for different message types.
//!
//! This example shows how to use different checksum sizes based on message characteristics:
//! - CRC16 (2 bytes) for small, high-frequency messages
//! - CRC32 (4 bytes) for medium-sized messages
//! - XXHash64 (8 bytes) for large, critical messages

use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::fs::File;
#[allow(unused_imports)]
use std::io::{BufReader, BufWriter};

// Import framing types when checksum features are enabled
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
use flatstream::framing::{ChecksumDeframer, ChecksumFramer};

// Define different message types for demonstration
#[derive(Debug)]
#[allow(dead_code)]
struct SmallMessage {
    sensor_id: u8,
    value: f32,
}

#[derive(Debug)]
#[allow(dead_code)]
struct MediumMessage {
    device_id: String,
    timestamp: u64,
    readings: Vec<f64>,
}

#[derive(Debug)]
#[allow(dead_code)]
struct LargeMessage {
    batch_id: String,
    metadata: String,
    data_points: Vec<f64>,
    flags: Vec<bool>,
}

impl StreamSerialize for SmallMessage {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let data = format!("{}", self.sensor_id);
        let data_str = builder.create_string(&data);
        builder.finish(data_str, None);
        Ok(())
    }
}

impl StreamSerialize for MediumMessage {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let data = format!("{},{},{}", self.device_id, self.timestamp, self.readings[0]);
        let data_str = builder.create_string(&data);
        builder.finish(data_str, None);
        Ok(())
    }
}

impl StreamSerialize for LargeMessage {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let data = format!(
            "{},{},{},{},{},{}",
            self.batch_id,
            self.metadata,
            self.data_points[0],
            self.flags[0],
            self.data_points[1],
            self.flags[1]
        );
        let data_str = builder.create_string(&data);
        builder.finish(data_str, None);
        Ok(())
    }
}

fn main() -> Result<()> {
    println!("=== Sized Checksums Example (v2.5) ===\n");

    // Create test messages
    let small_messages = vec![
        SmallMessage {
            sensor_id: 1,
            value: 23.5,
        },
        SmallMessage {
            sensor_id: 2,
            value: 24.1,
        },
        SmallMessage {
            sensor_id: 3,
            value: 22.8,
        },
    ];

    let medium_messages = vec![
        MediumMessage {
            device_id: "device-alpha".to_string(),
            timestamp: 1234567890,
            readings: vec![1.1, 2.2, 3.3],
        },
        MediumMessage {
            device_id: "device-beta".to_string(),
            timestamp: 1234567891,
            readings: vec![4.4, 5.5, 6.6],
        },
    ];

    let large_messages = vec![
        LargeMessage {
            batch_id: "batch-001".to_string(),
            metadata: "High-precision sensor data".to_string(),
            data_points: (0..1000).map(|i| i as f64 * 0.1).collect(),
            flags: (0..100).map(|i| i % 2 == 0).collect(),
        },
        LargeMessage {
            batch_id: "batch-002".to_string(),
            metadata: "Calibration data".to_string(),
            data_points: (0..2000).map(|i| i as f64 * 0.05).collect(),
            flags: (0..200).map(|i| i % 3 == 0).collect(),
        },
    ];

    // Demonstrate different checksum sizes
    demonstrate_checksum_sizes(&small_messages, &medium_messages, &large_messages)?;

    // Show performance comparison
    demonstrate_performance_comparison()?;

    println!("✅ Sized checksums example completed successfully!");
    Ok(())
}

fn demonstrate_checksum_sizes(
    #[allow(unused_variables)] small_messages: &[SmallMessage],
    #[allow(unused_variables)] medium_messages: &[MediumMessage],
    #[allow(unused_variables)] large_messages: &[LargeMessage],
) -> Result<()> {
    println!("1. Checksum Size Comparison...");

    // Small messages with CRC16 (2 bytes)
    #[cfg(feature = "crc16")]
    {
        println!("   Small messages with CRC16 (2 bytes):");
        let file = File::create("small_messages_crc16.bin")?;
        let writer = BufWriter::new(file);
        let checksum = Crc16::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(writer, framer);
        let mut builder = FlatBufferBuilder::new();

        for message in small_messages {
            builder.reset();
            message.serialize(&mut builder)?;
            writer.write_finished(&mut builder)?;
        }
        writer.flush()?;

        let file_size = std::fs::metadata("small_messages_crc16.bin")?.len();
        println!("     File size: {} bytes", file_size);
        println!(
            "     Overhead: ~{} bytes per message",
            file_size / small_messages.len() as u64
        );
    }

    // Medium messages with CRC32 (4 bytes)
    #[cfg(feature = "crc32")]
    {
        println!("   Medium messages with CRC32 (4 bytes):");
        let file = File::create("medium_messages_crc32.bin")?;
        let writer = BufWriter::new(file);
        let checksum = Crc32::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(writer, framer);
        let mut builder = FlatBufferBuilder::new();

        for message in medium_messages {
            builder.reset();
            message.serialize(&mut builder)?;
            writer.write_finished(&mut builder)?;
        }
        writer.flush()?;

        let file_size = std::fs::metadata("medium_messages_crc32.bin")?.len();
        println!("     File size: {} bytes", file_size);
        println!(
            "     Overhead: ~{} bytes per message",
            file_size / medium_messages.len() as u64
        );
    }

    // Large messages with XXHash64 (8 bytes)
    #[cfg(feature = "xxhash")]
    {
        println!("   Large messages with XXHash64 (8 bytes):");
        let file = File::create("large_messages_xxhash64.bin")?;
        let writer = BufWriter::new(file);
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(writer, framer);
        let mut builder = FlatBufferBuilder::new();

        for message in large_messages {
            builder.reset();
            message.serialize(&mut builder)?;
            writer.write_finished(&mut builder)?;
        }
        writer.flush()?;

        let file_size = std::fs::metadata("large_messages_xxhash64.bin")?.len();
        println!("     File size: {} bytes", file_size);
        println!(
            "     Overhead: ~{} bytes per message",
            file_size / large_messages.len() as u64
        );
    }

    // Read back using processor API
    println!("\n   Reading messages back with processor API:");

    #[cfg(feature = "crc16")]
    {
        let file = File::open("small_messages_crc16.bin")?;
        let reader = BufReader::new(file);
        let checksum = Crc16::new();
        let deframer = ChecksumDeframer::new(checksum);
        let mut reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        reader.process_all(|payload| {
            count += 1;
            if let Ok(message) = std::str::from_utf8(payload) {
                println!("     Small message {}: {}", count, message);
            }
            Ok(())
        })?;
        println!(
            "     ✓ Read {} small messages with CRC16 verification",
            count
        );
    }

    #[cfg(feature = "crc32")]
    {
        let file = File::open("medium_messages_crc32.bin")?;
        let reader = BufReader::new(file);
        let checksum = Crc32::new();
        let deframer = ChecksumDeframer::new(checksum);
        let mut reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        reader.process_all(|payload| {
            count += 1;
            if let Ok(message) = std::str::from_utf8(payload) {
                println!("     Medium message {}: {}", count, message);
            }
            Ok(())
        })?;
        println!(
            "     ✓ Read {} medium messages with CRC32 verification",
            count
        );
    }

    #[cfg(feature = "xxhash")]
    {
        let file = File::open("large_messages_xxhash64.bin")?;
        let reader = BufReader::new(file);
        let checksum = XxHash64::new();
        let deframer = ChecksumDeframer::new(checksum);
        let mut reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        reader.process_all(|payload| {
            count += 1;
            if let Ok(message) = std::str::from_utf8(payload) {
                println!("     Large message {}: {}", count, message);
            }
            Ok(())
        })?;
        println!(
            "     ✓ Read {} large messages with XXHash64 verification",
            count
        );
    }

    println!("   ✓ Checksum size comparison completed\n");
    Ok(())
}

fn demonstrate_performance_comparison() -> Result<()> {
    println!("2. Performance Comparison...");
    println!("   Note: This measures write performance including checksum computation\n");

    let num_messages = 10_000;
    let test_message = "performance-test-message";

    // External builder management
    #[allow(unused_variables)]
    let mut builder = FlatBufferBuilder::new();

    // No checksum (baseline)
    {
        let file = File::create("performance_no_checksum.bin")?;
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(writer, framer);

        let start = std::time::Instant::now();
        for _ in 0..num_messages {
            builder.reset();
            let data = builder.create_string(test_message);
            builder.finish(data, None);
            writer.write_finished(&mut builder)?;
        }
        writer.flush()?;
        let duration = start.elapsed();

        let file_size = std::fs::metadata("performance_no_checksum.bin")?.len();
        println!("   No checksum: {:?}, {} bytes", duration, file_size);
    }

    // CRC16 performance
    #[cfg(feature = "crc16")]
    {
        let file = File::create("performance_crc16.bin")?;
        let writer = BufWriter::new(file);
        let checksum = Crc16::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(writer, framer);

        let start = std::time::Instant::now();
        for _ in 0..num_messages {
            builder.reset();
            let data = builder.create_string(test_message);
            builder.finish(data, None);
            writer.write_finished(&mut builder)?;
        }
        writer.flush()?;
        let duration = start.elapsed();

        let file_size = std::fs::metadata("performance_crc16.bin")?.len();
        println!("   CRC16:      {:?}, {} bytes", duration, file_size);
    }

    // CRC32 performance
    #[cfg(feature = "crc32")]
    {
        let file = File::create("performance_crc32.bin")?;
        let writer = BufWriter::new(file);
        let checksum = Crc32::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(writer, framer);

        let start = std::time::Instant::now();
        for _ in 0..num_messages {
            builder.reset();
            let data = builder.create_string(test_message);
            builder.finish(data, None);
            writer.write_finished(&mut builder)?;
        }
        writer.flush()?;
        let duration = start.elapsed();

        let file_size = std::fs::metadata("performance_crc32.bin")?.len();
        println!("   CRC32:      {:?}, {} bytes", duration, file_size);
    }

    // XXHash64 performance
    #[cfg(feature = "xxhash")]
    {
        let file = File::create("performance_xxhash64.bin")?;
        let writer = BufWriter::new(file);
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut writer = StreamWriter::new(writer, framer);

        let start = std::time::Instant::now();
        for _ in 0..num_messages {
            builder.reset();
            let data = builder.create_string(test_message);
            builder.finish(data, None);
            writer.write_finished(&mut builder)?;
        }
        writer.flush()?;
        let duration = start.elapsed();

        let file_size = std::fs::metadata("performance_xxhash64.bin")?.len();
        println!("   XXHash64:   {:?}, {} bytes", duration, file_size);
    }

    println!("   ✓ Performance comparison completed\n");
    Ok(())
}
