use flatstream::*;

#[cfg(feature = "crc32")]
use flatbuffers::FlatBufferBuilder;
#[cfg(feature = "crc32")]
use std::fs::File;
#[cfg(feature = "crc32")]
use std::io::{BufReader, BufWriter};

// Import the framing types directly since we need them for CRC32
#[cfg(feature = "crc32")]
use flatstream::framing::{ChecksumDeframer, ChecksumFramer};

// This example demonstrates how easy it is to add new checksum algorithms
// to the flatstream-rs library using the v2.5 trait-based architecture.

fn main() -> Result<()> {
    println!("=== CRC32 Checksum Example (v2.5) ===\n");

    // This example requires the "crc32" feature to be enabled
    // Run with: cargo run --example crc32_example --features crc32

    #[cfg(feature = "crc32")]
    {
        println!("1. Writing data with CRC32 checksum:");
        let file = File::create("crc32_data.bin")?;
        let writer = BufWriter::new(file);

        // Use CRC32 checksum for data integrity
        let checksum_alg = Crc32::new();
        let framer = ChecksumFramer::new(checksum_alg);
        let mut stream_writer = StreamWriter::new(writer, framer);

        // External builder management for zero-allocation writes
        let mut builder = FlatBufferBuilder::new();

        // Write some test data
        let test_messages = [
            "Hello, CRC32!",
            "This is a test message",
            "CRC32 provides error detection",
            "Perfect for network transmission",
        ];

        for (i, message) in test_messages.iter().enumerate() {
            // Build and write with external builder
            builder.reset();
            let data = builder.create_string(message);
            builder.finish(data, None);
            stream_writer.write_finished(&mut builder)?;
            println!("  Wrote message {}: '{}'", i + 1, message);
        }

        stream_writer.flush()?;
        println!(
            "  ✓ Wrote {} messages with CRC32 checksums\n",
            test_messages.len()
        );

        // Read back and verify using processor API
        println!("2. Reading data with CRC32 verification:");
        let file = File::open("crc32_data.bin")?;
        let reader = BufReader::new(file);

        let checksum_alg = Crc32::new();
        let deframer = ChecksumDeframer::new(checksum_alg);
        let mut stream_reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        stream_reader.process_all(|payload| {
            count += 1;
            if let Ok(message) = std::str::from_utf8(payload) {
                println!("  Read message {count}: '{message}' (CRC32 verified)");
            } else {
                println!(
                    "  Read message {}: {} bytes (CRC32 verified)",
                    count,
                    payload.len()
                );
            }
            Ok(())
        })?;

        println!("  ✓ Successfully read {count} messages with CRC32 verification\n");

        // Demonstrate corruption detection
        println!("3. Testing corruption detection:");
        {
            let mut data = std::fs::read("crc32_data.bin")?;
            if data.len() > 20 {
                // Corrupt a byte in the middle of the first message
                data[20] ^= 1; // Flip one bit
                std::fs::write("crc32_corrupted.bin", data)?;
            }

            let file = File::open("crc32_corrupted.bin")?;
            let reader = BufReader::new(file);
            let checksum_alg = Crc32::new();
            let deframer = ChecksumDeframer::new(checksum_alg);
            let mut stream_reader = StreamReader::new(reader, deframer);

            let result = stream_reader.read_message();
            match result {
                Ok(_) => {
                    println!("  ⚠️  Unexpected: corruption not detected!");
                }
                Err(e) => {
                    println!("  ✅ Corruption detected: {e}");
                }
            }
        }

        println!("  ✓ CRC32 corruption detection test completed\n");
    }

    #[cfg(not(feature = "crc32"))]
    {
        println!("CRC32 feature not enabled.");
        println!("To run this example with CRC32 support, use:");
        println!("  cargo run --example crc32_example --features crc32");
    }

    println!("=== CRC32 Example Complete ===");
    println!("Key v2.5 features demonstrated:");
    println!("  • External builder management for zero-allocation writes");
    println!("  • Processor API for high-performance bulk processing");
    println!("  • CRC32 checksum for data integrity");
    println!("  • Zero-copy message processing throughout");

    Ok(())
}
