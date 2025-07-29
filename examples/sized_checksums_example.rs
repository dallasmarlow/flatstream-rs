//! Example demonstrating sized checksums for different message types.
//!
//! This example shows how to use different checksum sizes based on message characteristics:
//! - CRC16 (2 bytes) for small, high-frequency messages
//! - CRC32 (4 bytes) for medium-sized messages
//! - XXHash64 (8 bytes) for large, critical messages

use flatstream_rs::*;
use std::fs::File;
use std::io::{BufReader, BufWriter};

// Import framing types when checksum features are enabled
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
use flatstream_rs::framing::{ChecksumDeframer, ChecksumFramer};

// Define different message types for demonstration
#[derive(Debug)]
struct SmallMessage {
    sensor_id: u8,
    value: f32,
}

#[derive(Debug)]
struct MediumMessage {
    device_id: String,
    timestamp: u64,
    readings: Vec<f64>,
}

#[derive(Debug)]
struct LargeMessage {
    batch_id: String,
    metadata: String,
    data_points: Vec<f64>,
    flags: Vec<bool>,
}

impl StreamSerialize for SmallMessage {
    fn serialize(&self, builder: &mut flatbuffers::FlatBufferBuilder) -> Result<()> {
        // Simple serialization for small messages
        let sensor_id = builder.create_string(&format!("sensor-{}", self.sensor_id));
        builder.finish(sensor_id, None);
        Ok(())
    }
}

impl StreamSerialize for MediumMessage {
    fn serialize(&self, builder: &mut flatbuffers::FlatBufferBuilder) -> Result<()> {
        // Medium complexity serialization
        let device_id = builder.create_string(&self.device_id);
        builder.finish(device_id, None);
        Ok(())
    }
}

impl StreamSerialize for LargeMessage {
    fn serialize(&self, builder: &mut flatbuffers::FlatBufferBuilder) -> Result<()> {
        // Complex serialization for large messages
        let batch_id = builder.create_string(&self.batch_id);
        let metadata = builder.create_string(&self.metadata);
        builder.finish(batch_id, None);
        Ok(())
    }
}

fn main() -> Result<()> {
    println!("=== Sized Checksums Example ===\n");

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

    let large_messages = vec![LargeMessage {
        batch_id: "batch-001".to_string(),
        metadata: "High-precision sensor data from industrial monitoring system".to_string(),
        data_points: (0..100).map(|i| i as f64 * 0.1).collect(),
        flags: (0..50).map(|i| i % 2 == 0).collect(),
    }];

    // Demonstrate different checksum sizes for different message types
    demonstrate_checksum_sizes(&small_messages, &medium_messages, &large_messages)?;

    // Show performance comparison
    demonstrate_performance_comparison()?;

    println!("✅ Sized checksums example completed successfully!");
    Ok(())
}

fn demonstrate_checksum_sizes(
    small_messages: &[SmallMessage],
    medium_messages: &[MediumMessage],
    large_messages: &[LargeMessage],
) -> Result<()> {
    println!("1. Writing messages with different checksum sizes...");

    // Write small messages with CRC16 (2 bytes)
    #[cfg(feature = "crc16")]
    {
        let file = File::create("small_messages_crc16.bin")?;
        let writer = BufWriter::new(file);
        let framer = ChecksumFramer::new(Crc16::new());
        let mut stream_writer = StreamWriter::new(writer, framer);

        for msg in small_messages {
            stream_writer.write(msg)?;
        }
        stream_writer.flush()?;

        let file_size = std::fs::metadata("small_messages_crc16.bin")?.len();
        println!("   Small messages (CRC16): {} bytes", file_size);
    }

    // Write medium messages with CRC32 (4 bytes)
    #[cfg(feature = "crc32")]
    {
        let file = File::create("medium_messages_crc32.bin")?;
        let writer = BufWriter::new(file);
        let framer = ChecksumFramer::new(Crc32::new());
        let mut stream_writer = StreamWriter::new(writer, framer);

        for msg in medium_messages {
            stream_writer.write(msg)?;
        }
        stream_writer.flush()?;

        let file_size = std::fs::metadata("medium_messages_crc32.bin")?.len();
        println!("   Medium messages (CRC32): {} bytes", file_size);
    }

    // Write large messages with XXHash64 (8 bytes)
    #[cfg(feature = "xxhash")]
    {
        let file = File::create("large_messages_xxhash64.bin")?;
        let writer = BufWriter::new(file);
        let framer = ChecksumFramer::new(XxHash64::new());
        let mut stream_writer = StreamWriter::new(writer, framer);

        for msg in large_messages {
            stream_writer.write(msg)?;
        }
        stream_writer.flush()?;

        let file_size = std::fs::metadata("large_messages_xxhash64.bin")?.len();
        println!("   Large messages (XXHash64): {} bytes", file_size);
    }

    // Read back and verify
    println!("\n2. Reading and verifying messages...");

    #[cfg(feature = "crc16")]
    {
        let file = File::open("small_messages_crc16.bin")?;
        let reader = BufReader::new(file);
        let deframer = ChecksumDeframer::new(Crc16::new());
        let stream_reader = StreamReader::new(reader, deframer);
        let count = stream_reader.count();
        println!("   Read {} small messages with CRC16 verification", count);
    }

    #[cfg(feature = "crc32")]
    {
        let file = File::open("medium_messages_crc32.bin")?;
        let reader = BufReader::new(file);
        let deframer = ChecksumDeframer::new(Crc32::new());
        let stream_reader = StreamReader::new(reader, deframer);
        let count = stream_reader.count();
        println!("   Read {} medium messages with CRC32 verification", count);
    }

    #[cfg(feature = "xxhash")]
    {
        let file = File::open("large_messages_xxhash64.bin")?;
        let reader = BufReader::new(file);
        let deframer = ChecksumDeframer::new(XxHash64::new());
        let stream_reader = StreamReader::new(reader, deframer);
        let count = stream_reader.count();
        println!(
            "   Read {} large messages with XXHash64 verification",
            count
        );
    }

    Ok(())
}

fn demonstrate_performance_comparison() -> Result<()> {
    println!("\n3. Performance comparison...");

    let test_data = "This is a test message for performance comparison";
    let iterations = 1000;

    // Test with no checksum
    {
        let start = std::time::Instant::now();
        let file = File::create("performance_no_checksum.bin")?;
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut stream_writer = StreamWriter::new(writer, framer);

        for _ in 0..iterations {
            stream_writer.write(&test_data)?;
        }
        stream_writer.flush()?;

        let duration = start.elapsed();
        let file_size = std::fs::metadata("performance_no_checksum.bin")?.len();
        println!(
            "   No checksum: {} messages in {:?}, {} bytes",
            iterations, duration, file_size
        );
    }

    // Test with CRC16
    #[cfg(feature = "crc16")]
    {
        let start = std::time::Instant::now();
        let file = File::create("performance_crc16.bin")?;
        let writer = BufWriter::new(file);
        let framer = ChecksumFramer::new(Crc16::new());
        let mut stream_writer = StreamWriter::new(writer, framer);

        for _ in 0..iterations {
            stream_writer.write(&test_data)?;
        }
        stream_writer.flush()?;

        let duration = start.elapsed();
        let file_size = std::fs::metadata("performance_crc16.bin")?.len();
        println!(
            "   CRC16: {} messages in {:?}, {} bytes",
            iterations, duration, file_size
        );
    }

    // Test with CRC32
    #[cfg(feature = "crc32")]
    {
        let start = std::time::Instant::now();
        let file = File::create("performance_crc32.bin")?;
        let writer = BufWriter::new(file);
        let framer = ChecksumFramer::new(Crc32::new());
        let mut stream_writer = StreamWriter::new(writer, framer);

        for _ in 0..iterations {
            stream_writer.write(&test_data)?;
        }
        stream_writer.flush()?;

        let duration = start.elapsed();
        let file_size = std::fs::metadata("performance_crc32.bin")?.len();
        println!(
            "   CRC32: {} messages in {:?}, {} bytes",
            iterations, duration, file_size
        );
    }

    // Test with XXHash64
    #[cfg(feature = "xxhash")]
    {
        let start = std::time::Instant::now();
        let file = File::create("performance_xxhash64.bin")?;
        let writer = BufWriter::new(file);
        let framer = ChecksumFramer::new(XxHash64::new());
        let mut stream_writer = StreamWriter::new(writer, framer);

        for _ in 0..iterations {
            stream_writer.write(&test_data)?;
        }
        stream_writer.flush()?;

        let duration = start.elapsed();
        let file_size = std::fs::metadata("performance_xxhash64.bin")?.len();
        println!(
            "   XXHash64: {} messages in {:?}, {} bytes",
            iterations, duration, file_size
        );
    }

    println!("\n4. Checksum size comparison:");
    println!("   No checksum: 0 bytes overhead");
    #[cfg(feature = "crc16")]
    println!("   CRC16: 2 bytes overhead per message");
    #[cfg(feature = "crc32")]
    println!("   CRC32: 4 bytes overhead per message");
    #[cfg(feature = "xxhash")]
    println!("   XXHash64: 8 bytes overhead per message");

    println!("\n💡 Key Benefits:");
    println!(
        "   • CRC16: Perfect for high-frequency small messages (75% less overhead than XXHash64)"
    );
    println!(
        "   • CRC32: Good balance for medium-sized messages (50% less overhead than XXHash64)"
    );
    println!("   • XXHash64: Best for large, critical messages (maximum integrity)");

    println!("   • All checksums are pluggable and composable");

    Ok(())
}
