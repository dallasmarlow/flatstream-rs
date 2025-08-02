use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::time::Instant;

// This example demonstrates the high-performance optimizations available in flatstream-rs v2.5:
// 1. External builder management for zero-allocation writes
// 2. Processor API for zero-allocation reading

fn main() -> Result<()> {
    println!("=== High-Performance flatstream-rs v2.5 Example ===\n");

    let data_file = "performance_test.bin";
    let num_messages = 10_000;

    // Example 1: External Builder Management Performance Test
    println!("1. External Builder Management Performance Test:");
    {
        // Prepare a batch of messages
        let messages: Vec<String> = (0..num_messages)
            .map(|i| format!("high-frequency-message-{}", i))
            .collect();

        // Test iterative writing with external builder management (v2.5 pattern)
        let start = Instant::now();
        {
            let file = File::create(data_file)?;
            let writer = BufWriter::new(file);
            let framer = DefaultFramer;
            let mut stream_writer = StreamWriter::new(writer, framer);

            // External builder management for zero-allocation writes
            let mut builder = FlatBufferBuilder::new();
            for message in &messages {
                builder.reset();
                let data = builder.create_string(message);
                builder.finish(data, None);
                stream_writer.write_finished(&mut builder)?;
            }
            stream_writer.flush()?;
        }
        let v2_5_time = start.elapsed();

        println!("  v2.5 external builder: {:?}", v2_5_time);
        println!("  ✓ External builder management optimization demonstrated\n");
    }

    // Example 2: Processor API Reading Performance Test
    println!("2. Processor API Reading Performance Test:");
    {
        // Test processor API reading (v2.5 - zero-allocation)
        let start = Instant::now();
        {
            let file = File::open(data_file)?;
            let reader = BufReader::new(file);
            let deframer = DefaultDeframer;
            let mut stream_reader = StreamReader::new(reader, deframer);

            let mut count = 0;
            let mut total_size = 0;

            // Use the high-performance processor API
            stream_reader.process_all(|payload| {
                total_size += payload.len();
                count += 1;
                Ok(())
            })?;

            println!(
                "  Processor API reading: {} messages, {} total bytes",
                count, total_size
            );
        }
        let processor_time = start.elapsed();

        // Test expert API reading (v2.5 - manual control)
        let start = Instant::now();
        {
            let file = File::open(data_file)?;
            let reader = BufReader::new(file);
            let deframer = DefaultDeframer;
            let mut stream_reader = StreamReader::new(reader, deframer);

            let mut count = 0;
            let mut total_size = 0;

            // Use the expert API for manual control
            let mut messages = stream_reader.messages();
            while let Some(payload) = messages.next()? {
                total_size += payload.len();
                count += 1;
            }

            println!(
                "  Expert API reading: {} messages, {} total bytes",
                count, total_size
            );
        }
        let expert_time = start.elapsed();

        println!("  Processor API: {:?}", processor_time);
        println!("  Expert API:    {:?}", expert_time);
        println!("  ✓ Zero-allocation reading optimization demonstrated\n");
    }

    // Example 3: Real-world Data Processing
    println!("3. Real-world Data Processing Example:");
    {
        // Define a realistic data structure
        #[derive(Debug)]
        struct SensorData {
            timestamp: u64,
            sensor_id: u32,
            value: f64,
            unit: String, // Added unit field for the new serialize method
        }

        impl StreamSerialize for SensorData {
            fn serialize<A: flatbuffers::Allocator>(
                &self,
                builder: &mut FlatBufferBuilder<A>,
            ) -> Result<()> {
                let data = format!(
                    "{},{},{},{}",
                    self.sensor_id, self.timestamp, self.value, &self.unit
                );
                let data_str = builder.create_string(&data);
                builder.finish(data_str, None);
                Ok(())
            }
        }

        // Generate realistic sensor data
        let sensor_data: Vec<SensorData> = (0..1000)
            .map(|i| SensorData {
                timestamp: 1640995200000 + (i * 1000), // Unix timestamp in ms
                sensor_id: (i % 10) as u32,            // 10 different sensors
                value: 20.0 + (i as f64 * 0.1),        // Temperature-like values
                unit: "C".to_string(),                 // Added unit field
            })
            .collect();

        // Write sensor data using v2.5 pattern
        let sensor_file = "sensor_data.bin";
        println!("  Writing {} sensor readings...", sensor_data.len());

        let start = Instant::now();
        {
            let file = File::create(sensor_file)?;
            let writer = BufWriter::new(file);
            let framer = DefaultFramer;
            let mut stream_writer = StreamWriter::new(writer, framer);

            // External builder management for optimal performance
            let mut builder = FlatBufferBuilder::new();
            for data in &sensor_data {
                builder.reset();
                data.serialize(&mut builder)?;
                stream_writer.write_finished(&mut builder)?;
            }
            stream_writer.flush()?;
        }
        let write_time = start.elapsed();

        // Read and process sensor data using processor API
        println!("  Reading and processing sensor data...");
        let start = Instant::now();
        {
            let file = File::open(sensor_file)?;
            let reader = BufReader::new(file);
            let deframer = DefaultDeframer;
            let mut stream_reader = StreamReader::new(reader, deframer);

            let mut count = 0;
            let mut total_value = 0.0;

            // Process all sensor readings with zero-allocation
            stream_reader.process_all(|_payload| {
                // In a real application, you would deserialize the FlatBuffer here
                // For this example, we just count and simulate processing
                count += 1;
                total_value += 20.0; // Simulate value extraction
                Ok(())
            })?;

            println!("  Processed {} sensor readings", count);
            println!("  Average value: {:.2}", total_value / count as f64);
        }
        let read_time = start.elapsed();

        println!("  Write time: {:?}", write_time);
        println!("  Read time:  {:?}", read_time);
        println!("  Total time: {:?}", write_time + read_time);
        println!("  ✓ Real-world data processing demonstrated\n");
    }

    println!("=== Performance Example Complete ===");
    println!("The v2.5 Processor API provides:");
    println!("  • Zero-allocation writes through external builder management");
    println!("  • Zero-allocation reads through the processor API");
    println!("  • Maximum performance for high-frequency data processing");
    println!("  • Explicit control over memory allocation patterns");

    Ok(())
}
