//! Example demonstrating expert mode with external builder management for high performance.
//!
//! This example shows the performance difference between:
//! - Simple mode: Using `write()` with internal builder management
//! - Expert mode: Using `write_finished()` with external builder management
//!
//! Expert mode is recommended for:
//! - High-frequency data capture
//! - Real-time systems
//! - Production telemetry agents
//! - Any scenario where maximum throughput is required

use flatbuffers::FlatBufferBuilder;
use flatstream_rs::*;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::time::Instant;

// Define a high-frequency event type
#[derive(Debug, Clone)]
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
        // Serialize as a simple string for this example
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
    println!("=== Expert Mode Performance Example ===\n");

    // Demonstrate the difference between simple and expert modes
    demonstrate_mode_comparison()?;

    // Show expert mode in a high-frequency scenario
    demonstrate_high_frequency_scenario()?;

    // Demonstrate best practices for expert mode
    demonstrate_best_practices()?;

    println!("✅ Expert mode example completed successfully!");
    Ok(())
}

fn demonstrate_mode_comparison() -> Result<()> {
    println!("1. Performance Comparison: Simple vs Expert Mode\n");

    let num_events = 10_000;
    let events: Vec<HighFrequencyEvent> = (0..num_events)
        .map(|i| HighFrequencyEvent {
            timestamp: i as u64,
            price: 100.0 + (i as f64 * 0.01),
            volume: 1000 + i as u32,
            symbol: format!("AAPL{}", i % 10),
        })
        .collect();

    // Simple mode test
    println!("   Testing SIMPLE mode (internal builder)...");
    let start = Instant::now();
    {
        let file = File::create("simple_mode.bin")?;
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(writer, framer);

        // Simple mode: writer manages builder internally
        for event in &events {
            writer.write(event)?;
        }
        writer.flush()?;
    }
    let simple_time = start.elapsed();
    let simple_throughput = num_events as f64 / simple_time.as_secs_f64();

    // Expert mode test
    println!("   Testing EXPERT mode (external builder)...");
    let start = Instant::now();
    {
        let file = File::create("expert_mode.bin")?;
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(writer, framer);

        // Expert mode: manage builder externally for maximum performance
        let mut builder = FlatBufferBuilder::new();

        for event in &events {
            builder.reset(); // Critical: reuse allocated memory!
            event.serialize(&mut builder)?;
            writer.write_finished(&mut builder)?;
        }
        writer.flush()?;
    }
    let expert_time = start.elapsed();
    let expert_throughput = num_events as f64 / expert_time.as_secs_f64();

    // Results
    println!("\n   === Results ===");
    println!("   Simple mode:");
    println!("     Time:       {:?}", simple_time);
    println!("     Throughput: {:.0} messages/sec", simple_throughput);
    println!(
        "     Latency:    {:.3} µs/msg",
        simple_time.as_micros() as f64 / num_events as f64
    );

    println!("\n   Expert mode:");
    println!("     Time:       {:?}", expert_time);
    println!("     Throughput: {:.0} messages/sec", expert_throughput);
    println!(
        "     Latency:    {:.3} µs/msg",
        expert_time.as_micros() as f64 / num_events as f64
    );

    println!(
        "\n   Performance gain: {:.1}x faster",
        expert_throughput / simple_throughput
    );
    println!("   ✓ Mode comparison completed\n");
    Ok(())
}

fn demonstrate_high_frequency_scenario() -> Result<()> {
    println!("2. High-Frequency Data Capture Scenario\n");

    // Simulate a high-frequency data capture system
    let file = File::create("high_frequency.bin")?;
    let writer = BufWriter::new(file);
    let framer = DefaultFramer;
    let mut writer = StreamWriter::new(writer, framer);

    // Expert mode with external builder for maximum performance
    let mut builder = FlatBufferBuilder::new();

    let start = Instant::now();
    let num_events = 100_000;

    println!("   Capturing {} high-frequency events...", num_events);

    // Generate high-frequency events
    for i in 0..num_events {
        let event = HighFrequencyEvent {
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64,
            price: 150.0 + (i as f64 * 0.001), // Small price movements
            volume: 100 + (i % 1000) as u32,
            symbol: format!("SYMBOL{}", i % 100), // 100 different symbols
        };

        // Expert mode pattern: reset, serialize, write
        builder.reset();
        event.serialize(&mut builder)?;
        writer.write_finished(&mut builder)?;
    }
    writer.flush()?;

    let write_time = start.elapsed();
    let throughput = num_events as f64 / write_time.as_secs_f64();

    println!("   Write performance:");
    println!("     Time:       {:?}", write_time);
    println!("     Throughput: {:.0} events/second", throughput);
    println!(
        "     Latency:    {:.3} µs per event",
        write_time.as_micros() as f64 / num_events as f64
    );

    // Read back using processor API for maximum performance
    println!("\n   Reading events with zero-copy processor API...");
    let start = Instant::now();
    {
        let file = File::open("high_frequency.bin")?;
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let mut reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        reader.process_all(|_payload| {
            count += 1;
            Ok(())
        })?;

        let read_time = start.elapsed();
        let read_throughput = count as f64 / read_time.as_secs_f64();

        println!("   Read performance:");
        println!("     Time:       {:?}", read_time);
        println!("     Throughput: {:.0} events/second", read_throughput);
        println!(
            "     Latency:    {:.3} µs per event",
            read_time.as_micros() as f64 / count as f64
        );
    }

    println!("   ✓ High-frequency scenario completed\n");
    Ok(())
}

fn demonstrate_best_practices() -> Result<()> {
    println!("3. Expert Mode Best Practices\n");

    // Best Practice 1: Builder reuse patterns
    println!("   a) Builder Reuse Pattern:");
    {
        let file = File::create("best_practices.bin")?;
        let writer = BufWriter::new(file);
        let mut writer = StreamWriter::new(writer, DefaultFramer);

        // GOOD: Single builder, reused via reset()
        let mut builder = FlatBufferBuilder::new();

        println!("      ✓ Create builder once, outside the loop");

        for i in 0..100 {
            let event = HighFrequencyEvent {
                timestamp: i,
                price: 100.0 + i as f64,
                volume: 100,
                symbol: "TEST".to_string(),
            };

            // GOOD: Reset reuses existing allocations
            builder.reset();
            event.serialize(&mut builder)?;
            writer.write_finished(&mut builder)?;
        }

        println!("      ✓ Reset builder for each message (reuses memory)");
        println!("      ✓ Use write_finished() for maximum control");
    }

    // Best Practice 2: Buffered I/O
    println!("\n   b) Always Use Buffered I/O:");
    {
        // GOOD: Wrapped in BufWriter
        let file = File::create("buffered.bin")?;
        let buffered_writer = BufWriter::new(file);
        let _writer = StreamWriter::new(buffered_writer, DefaultFramer);
        println!("      ✓ BufWriter reduces system calls");

        // BAD: Direct file handle (example only - don't do this!)
        // let file = File::create("unbuffered.bin")?;
        // let writer = StreamWriter::new(file, DefaultFramer);
        println!("      ✗ Avoid: Direct file handles cause excessive syscalls");
    }

    // Best Practice 3: Error handling
    println!("\n   c) Production Error Handling:");
    {
        let file = File::create("error_handling.bin")?;
        let writer = BufWriter::new(file);
        let mut writer = StreamWriter::new(writer, DefaultFramer);
        let mut builder = FlatBufferBuilder::new();

        let event = HighFrequencyEvent {
            timestamp: 123,
            price: 100.0,
            volume: 1000,
            symbol: "TEST".to_string(),
        };

        // Production pattern: handle errors appropriately
        match (|| -> Result<()> {
            builder.reset();
            event.serialize(&mut builder)?;
            writer.write_finished(&mut builder)?;
            writer.flush()?;
            Ok(())
        })() {
            Ok(()) => println!("      ✓ Message written successfully"),
            Err(Error::Io(e)) => eprintln!("      ✗ I/O error: {}", e),
            Err(e) => eprintln!("      ✗ Stream error: {}", e),
        }
    }

    println!("\n   ✓ Best practices demonstrated\n");
    Ok(())
}

// Note: For true zero-allocation systems, you would need to implement a custom
// allocator for FlatBufferBuilder. The flatbuffers crate supports custom allocators,
// but that's beyond the scope of this example. The expert mode shown here already
// provides excellent performance by reusing the builder's internal allocations.
