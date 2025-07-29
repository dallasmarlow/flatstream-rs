use flatstream_rs::*;


// Import the framing types directly since we need them for CRC32
#[cfg(feature = "crc32")]
use flatstream_rs::framing::{ChecksumDeframer, ChecksumFramer};

// This example demonstrates how easy it is to add new checksum algorithms
// to the flatstream-rs library using the v2 trait-based architecture.

fn main() -> Result<()> {
    println!("=== CRC32 Checksum Example ===\n");

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

        // Write some test data
        let test_messages = vec![
            "Hello, CRC32!",
            "This is a test message",
            "CRC32 provides error detection",
            "Perfect for network transmission",
        ];

        for (i, message) in test_messages.iter().enumerate() {
            stream_writer.write(message)?;
            println!("  Wrote message {}: '{}'", i + 1, message);
        }

        stream_writer.flush()?;
        println!(
            "  ✓ Wrote {} messages with CRC32 checksums\n",
            test_messages.len()
        );

        // Read back and verify
        println!("2. Reading data with CRC32 verification:");
        let file = File::open("crc32_data.bin")?;
        let reader = BufReader::new(file);

        let checksum_alg = Crc32::new();
        let deframer = ChecksumDeframer::new(checksum_alg);
        let stream_reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        for result in stream_reader {
            match result {
                Ok(payload) => {
                    count += 1;
                    println!(
                        "  Read message {}: {} bytes (CRC32 verified)",
                        count,
                        payload.len()
                    );
                }
                Err(e) => {
                    eprintln!("  Error reading message: {}", e);
                    break;
                }
            }
        }

        println!(
            "  ✓ Successfully read {} messages with CRC32 verification\n",
            count
        );

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
                    println!("  ✓ Corruption detected: {}", e);
                }
            }
        }

        println!("\n=== Example Complete ===");
        println!("Files created:");
        println!("  - crc32_data.bin (with CRC32 checksums)");
        println!("  - crc32_corrupted.bin (for corruption testing)");
        println!("\nKey benefits demonstrated:");
        println!("  - Easy addition of new checksum algorithms");
        println!("  - Clean separation of concerns");
        println!("  - Composable architecture");
        println!("  - Feature-gated dependencies");
    }

    #[cfg(not(feature = "crc32"))]
    {
        println!("❌ CRC32 feature not enabled!");
        println!("To run this example, use:");
        println!("  cargo run --example crc32_example --features crc32");
        println!("\nThis demonstrates how the library can be extended");
        println!("with new checksum algorithms without modifying core code.");
    }

    Ok(())
}
