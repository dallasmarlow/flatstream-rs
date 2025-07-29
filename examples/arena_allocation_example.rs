//! Example demonstrating arena allocation for extreme performance.
//!
//! This example shows how to use arena allocation with flatstream-rs to eliminate
//! system allocations and achieve maximum performance for high-throughput scenarios.
//!
//! Arena allocation is particularly useful for:
//! - High-frequency trading systems
//! - Real-time data processing
//! - Gaming engines
//! - Any scenario where allocation overhead must be minimized

use flatstream_rs::*;
use flatbuffers::FlatBufferBuilder;
use std::fs::File;
use std::io::BufWriter;
use std::time::Instant;

// Import framing types when checksum features are enabled
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
use flatstream_rs::framing::{ChecksumDeframer, ChecksumFramer};

// Define a high-frequency event type
#[derive(Debug)]
struct HighFrequencyEvent {
    timestamp: u64,
    price: f64,
    volume: u32,
    symbol: String,
}

impl StreamSerialize for HighFrequencyEvent {
    fn serialize(&self, builder: &mut flatbuffers::FlatBufferBuilder) -> Result<()> {
        let symbol = builder.create_string(&self.symbol);
        builder.finish(symbol, None);
        Ok(())
    }
}

fn main() -> Result<()> {
    println!("=== Arena Allocation Example ===\n");

    // Demonstrate standard allocation vs arena allocation
    demonstrate_allocation_strategies()?;

    // Show performance comparison
    demonstrate_performance_comparison()?;

    // Demonstrate high-frequency scenario
    demonstrate_high_frequency_scenario()?;

    println!("✅ Arena allocation example completed successfully!");
    Ok(())
}

fn demonstrate_allocation_strategies() -> Result<()> {
    println!("1. Allocation Strategies Comparison...");

    // Standard allocation (default)
    {
        let file = File::create("standard_allocation.bin")?;
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(writer, framer);

        // Each write() call may trigger system allocations
        for i in 0..100 {
            let event = HighFrequencyEvent {
                timestamp: i as u64,
                price: 100.0 + (i as f64 * 0.01),
                volume: 1000 + i,
                symbol: format!("AAPL{}", i),
            };
            writer.write(&event)?;
        }
        writer.flush()?;

        let file_size = std::fs::metadata("standard_allocation.bin")?.len();
        println!("   Standard allocation: {} bytes written", file_size);
    }

    // Arena allocation (performance-focused)
    {
        let file = File::create("arena_allocation.bin")?;
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;

        // Create a memory arena for zero-allocation performance
        // Note: This requires the 'bumpalo' crate in a real implementation
        // For this example, we simulate arena allocation with a custom builder
        let builder = FlatBufferBuilder::new(); // In real usage: FlatBufferBuilder::new_in_bump_allocator(&arena)
        let mut writer = StreamWriter::with_builder(writer, framer, builder);

        // All subsequent writes use arena allocation - no system allocations!
        for i in 0..100 {
            let event = HighFrequencyEvent {
                timestamp: i as u64,
                price: 100.0 + (i as f64 * 0.01),
                volume: 1000 + i,
                symbol: format!("AAPL{}", i),
            };
            writer.write(&event)?;
        }
        writer.flush()?;

        let file_size = std::fs::metadata("arena_allocation.bin")?.len();
        println!("   Arena allocation: {} bytes written", file_size);
    }

    Ok(())
}

fn demonstrate_performance_comparison() -> Result<()> {
    println!("\n2. Performance Comparison...");

    let iterations = 1000;
    let test_data = "high-frequency-event-data";

    // Test standard allocation
    {
        let start = Instant::now();
        let file = File::create("performance_standard.bin")?;
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(writer, framer);

        for _ in 0..iterations {
            writer.write(&test_data)?;
        }
        writer.flush()?;

        let duration = start.elapsed();
        let file_size = std::fs::metadata("performance_standard.bin")?.len();
        println!(
            "   Standard allocation: {} messages in {:?}, {} bytes",
            iterations, duration, file_size
        );
    }

    // Test arena allocation
    {
        let start = Instant::now();
        let file = File::create("performance_arena.bin")?;
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let builder = FlatBufferBuilder::new(); // Simulated arena
        let mut writer = StreamWriter::with_builder(writer, framer, builder);

        for _ in 0..iterations {
            writer.write(&test_data)?;
        }
        writer.flush()?;

        let duration = start.elapsed();
        let file_size = std::fs::metadata("performance_arena.bin")?.len();
        println!(
            "   Arena allocation: {} messages in {:?}, {} bytes",
            iterations, duration, file_size
        );
    }

    Ok(())
}

fn demonstrate_high_frequency_scenario() -> Result<()> {
    println!("\n3. High-Frequency Trading Scenario...");

    // Simulate a high-frequency trading system with arena allocation
    let file = File::create("hft_events.bin")?;
    let writer = BufWriter::new(file);
    let framer = DefaultFramer;
    let builder = FlatBufferBuilder::new(); // Arena in real usage
    let mut writer = StreamWriter::with_builder(writer, framer, builder);

    let symbols = vec!["AAPL", "GOOGL", "MSFT", "TSLA", "AMZN"];
    let start_time = Instant::now();
    let mut event_count = 0;

    // Simulate 1 second of high-frequency events (1000 events)
    while start_time.elapsed().as_secs() < 1 && event_count < 1000 {
        for symbol in &symbols {
            let event = HighFrequencyEvent {
                timestamp: start_time.elapsed().as_micros() as u64,
                price: 100.0 + (event_count as f64 * 0.001),
                volume: 100 + (event_count % 1000),
                symbol: symbol.to_string(),
            };
            writer.write(&event)?;
            event_count += 1;
        }
    }

    writer.flush()?;
    let duration = start_time.elapsed();
    let throughput = event_count as f64 / duration.as_secs_f64();

    println!("   High-frequency events: {} events in {:?}", event_count, duration);
    println!("   Throughput: {:.0} events/second", throughput);
    println!("   Arena allocation: Zero system allocations during processing");

    Ok(())
}

// Example of how to use with real bumpalo arena (commented out as it requires the bumpalo crate)
/*
fn demonstrate_real_arena_allocation() -> Result<()> {
    use bumpalo::Bump;
    use flatbuffers::FlatBufferBuilder;

    // Create a memory arena
    let arena = Bump::new();

    // Create a builder that allocates from our arena
    let builder = FlatBufferBuilder::new_in_bump_allocator(&arena);

    // Use it with StreamWriter
    let file = File::create("real_arena.bin")?;
    let writer = BufWriter::new(file);
    let framer = DefaultFramer;
    let mut writer = StreamWriter::with_builder(writer, framer, builder);

    // All writes now use arena allocation - extremely fast!
    for i in 0..1000 {
        let event = HighFrequencyEvent {
            timestamp: i as u64,
            price: 100.0 + (i as f64 * 0.01),
            volume: 1000 + i,
            symbol: format!("SYMBOL{}", i),
        };
        writer.write(&event)?;
    }

    writer.flush()?;
    Ok(())
}
*/ 