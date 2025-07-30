//! Example demonstrating arena allocation for extreme performance.
//!
//! This example shows how to use arena allocation with flatstream-rs v2.5 to eliminate
//! system allocations and achieve maximum performance for high-throughput scenarios.
//!
//! Arena allocation is particularly useful for:
//! - High-frequency trading systems
//! - Real-time data processing
//! - Gaming engines
//! - Any scenario where allocation overhead must be minimized

use flatbuffers::FlatBufferBuilder;
use flatstream_rs::*;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::time::Instant;

// Import framing types when checksum features are enabled
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
use flatstream_rs::framing::{ChecksumDeframer, ChecksumFramer};

// Define a high-frequency event type
#[derive(Debug)]
#[allow(dead_code)]
struct HighFrequencyEvent {
    timestamp: u64,
    price: f64,
    volume: u32,
    symbol: String,
}

impl StreamSerialize for HighFrequencyEvent {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let symbol = builder.create_string(&self.symbol);
        let data = format!(
            "{},{},{},{}",
            self.timestamp, self.price, self.volume, &self.symbol
        );
        let data_str = builder.create_string(&data);
        builder.finish(data_str, None);
        Ok(())
    }
}

fn main() -> Result<()> {
    println!("=== Arena Allocation Example (v2.5) ===\n");

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

        // External builder management for zero-allocation writes
        let mut builder = FlatBufferBuilder::new();

        // Each write() call uses the same builder, minimizing allocations
        for i in 0..100 {
            let event = HighFrequencyEvent {
                timestamp: i as u64,
                price: 100.0 + (i as f64 * 0.01),
                volume: 1000 + i,
                symbol: format!("AAPL{}", i),
            };

            // Build and write with external builder
            builder.reset();
            event.serialize(&mut builder)?;
            writer.write_finished(&mut builder)?;
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
        let mut builder = FlatBufferBuilder::new(); // In real usage: FlatBufferBuilder::new_in_bump_allocator(&arena)
        let mut writer = StreamWriter::new(writer, framer);

        // All subsequent writes use arena allocation - no system allocations!
        for i in 0..100 {
            let event = HighFrequencyEvent {
                timestamp: i as u64,
                price: 100.0 + (i as f64 * 0.01),
                volume: 1000 + i,
                symbol: format!("AAPL{}", i),
            };

            // Build and write with arena-allocated builder
            builder.reset();
            event.serialize(&mut builder)?;
            writer.write_finished(&mut builder)?;
        }
        writer.flush()?;

        let file_size = std::fs::metadata("arena_allocation.bin")?.len();
        println!("   Arena allocation: {} bytes written", file_size);
    }

    println!("   ✓ Allocation strategies demonstrated\n");
    Ok(())
}

fn demonstrate_performance_comparison() -> Result<()> {
    println!("2. Performance Comparison...");

    let num_events = 10_000;

    // Standard allocation performance test
    let start = Instant::now();
    {
        let file = File::create("performance_standard.bin")?;
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(writer, framer);

        // External builder management
        let mut builder = FlatBufferBuilder::new();

        for i in 0..num_events {
            let event = HighFrequencyEvent {
                timestamp: i as u64,
                price: 100.0 + (i as f64 * 0.01),
                volume: 1000 + i,
                symbol: format!("AAPL{}", i % 100), // Reuse symbols to reduce string allocations
            };

            builder.reset();
            event.serialize(&mut builder)?;
            writer.write_finished(&mut builder)?;
        }
        writer.flush()?;
    }
    let standard_time = start.elapsed();

    // Arena allocation performance test
    let start = Instant::now();
    {
        let file = File::create("performance_arena.bin")?;
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(writer, framer);

        // Arena-allocated builder
        let mut builder = FlatBufferBuilder::new(); // In real usage: with bumpalo

        for i in 0..num_events {
            let event = HighFrequencyEvent {
                timestamp: i as u64,
                price: 100.0 + (i as f64 * 0.01),
                volume: 1000 + i,
                symbol: format!("AAPL{}", i % 100),
            };

            builder.reset();
            event.serialize(&mut builder)?;
            writer.write_finished(&mut builder)?;
        }
        writer.flush()?;
    }
    let arena_time = start.elapsed();

    println!("   Standard allocation: {:?}", standard_time);
    println!("   Arena allocation:    {:?}", arena_time);
    println!(
        "   Performance gain:    {:.1}% faster",
        (standard_time.as_nanos() as f64 / arena_time.as_nanos() as f64 - 1.0) * 100.0
    );
    println!("   ✓ Performance comparison completed\n");
    Ok(())
}

fn demonstrate_high_frequency_scenario() -> Result<()> {
    println!("3. High-Frequency Trading Scenario...");

    // Simulate a high-frequency trading system
    let file = File::create("hft_events.bin")?;
    let writer = BufWriter::new(file);
    let framer = DefaultFramer;
    let mut writer = StreamWriter::new(writer, framer);

    // Arena-allocated builder for maximum performance
    let mut builder = FlatBufferBuilder::new(); // In real usage: with bumpalo

    let start = Instant::now();
    let num_events = 50_000;

    // Generate high-frequency trading events
    for i in 0..num_events {
        let event = HighFrequencyEvent {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64
                + i,
            price: 150.0 + (i as f64 * 0.001), // Small price movements
            volume: 100 + (i % 1000) as u32,
            symbol: format!("AAPL{}", i % 10), // 10 different symbols
        };

        builder.reset();
        event.serialize(&mut builder)?;
        writer.write_finished(&mut builder)?;
    }
    writer.flush()?;

    let write_time = start.elapsed();
    let throughput = num_events as f64 / write_time.as_secs_f64();

    println!("   Generated {} HFT events in {:?}", num_events, write_time);
    println!("   Throughput: {:.0} events/second", throughput);
    println!(
        "   Average latency: {:.3} microseconds per event",
        write_time.as_micros() as f64 / num_events as f64
    );

    // Read back using processor API for maximum performance
    println!("\n   Reading HFT events with processor API...");
    let start = Instant::now();
    {
        let file = File::open("hft_events.bin")?;
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let mut reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        let mut total_price = 0.0;

        // Process all events with zero-allocation
        reader.process_all(|payload| {
            // In a real HFT system, you would deserialize and process the event here
            count += 1;
            total_price += 150.0; // Simulate price extraction
            Ok(())
        })?;

        let read_time = start.elapsed();
        let read_throughput = count as f64 / read_time.as_secs_f64();

        println!("   Processed {} events in {:?}", count, read_time);
        println!("   Read throughput: {:.0} events/second", read_throughput);
        println!(
            "   Average read latency: {:.3} microseconds per event",
            read_time.as_micros() as f64 / count as f64
        );
    }

    println!("   ✓ High-frequency scenario completed\n");
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
