use flatstream_rs::*;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::time::Instant;

// This example demonstrates the high-performance optimizations available in flatstream-rs:
// 1. Write batching for efficient bulk writes
// 2. Zero-allocation reading for memory-efficient processing

fn main() -> Result<()> {
    println!("=== High-Performance flatstream-rs Example ===\n");

    let data_file = "performance_test.bin";
    let num_messages = 10_000;

    // Example 1: Write Batching Optimization
    println!("1. Write Batching Performance Test:");
    {
        // Prepare a batch of messages
        let messages: Vec<String> = (0..num_messages)
            .map(|i| format!("high-frequency-message-{}", i))
            .collect();

        // Test iterative writing (baseline)
        let start = Instant::now();
        {
            let file = File::create("iterative_test.bin")?;
            let writer = BufWriter::new(file);
            let framer = DefaultFramer;
            let mut stream_writer = StreamWriter::new(writer, framer);

            for message in &messages {
                stream_writer.write(message)?;
            }
            stream_writer.flush()?;
        }
        let iterative_time = start.elapsed();

        // Test batch writing (optimized)
        let start = Instant::now();
        {
            let file = File::create(data_file)?;
            let writer = BufWriter::new(file);
            let framer = DefaultFramer;
            let mut stream_writer = StreamWriter::new(writer, framer);

            // Use the new write_batch method
            stream_writer.write_batch(&messages)?;
            stream_writer.flush()?;
        }
        let batch_time = start.elapsed();

        println!("  Iterative writing: {:?}", iterative_time);
        println!("  Batch writing:     {:?}", batch_time);
        println!(
            "  Performance gain:  {:.1}% faster",
            (iterative_time.as_nanos() as f64 / batch_time.as_nanos() as f64 - 1.0) * 100.0
        );
        println!("  ✓ Write batching optimization demonstrated\n");
    }

    // Example 2: Zero-Allocation Reading Performance Test
    println!("2. Zero-Allocation Reading Performance Test:");
    {
        // Test iterator-based reading (baseline - involves allocations)
        let start = Instant::now();
        {
            let file = File::open(data_file)?;
            let reader = BufReader::new(file);
            let deframer = DefaultDeframer;
            let stream_reader = StreamReader::new(reader, deframer);

            let mut count = 0;
            let mut total_size = 0;
            for result in stream_reader {
                let payload = result?;
                total_size += payload.len();
                count += 1;
            }
            println!(
                "  Iterator reading:  {} messages, {} total bytes",
                count, total_size
            );
        }
        let iterator_time = start.elapsed();

        // Test zero-allocation reading (optimized)
        let start = Instant::now();
        {
            let file = File::open(data_file)?;
            let reader = BufReader::new(file);
            let deframer = DefaultDeframer;
            let mut stream_reader = StreamReader::new(reader, deframer);

            let mut count = 0;
            let mut total_size = 0;

            // Use the high-performance while let pattern
            while let Some(payload_slice) = stream_reader.read_message()? {
                // payload_slice is &[u8] - no allocation, just a borrow
                total_size += payload_slice.len();
                count += 1;
            }
            println!(
                "  Zero-copy reading: {} messages, {} total bytes",
                count, total_size
            );
        }
        let zero_copy_time = start.elapsed();

        println!("  Iterator reading:  {:?}", iterator_time);
        println!("  Zero-copy reading: {:?}", zero_copy_time);
        println!(
            "  Performance gain:  {:.1}% faster",
            (iterator_time.as_nanos() as f64 / zero_copy_time.as_nanos() as f64 - 1.0) * 100.0
        );
        println!("  ✓ Zero-allocation reading optimization demonstrated\n");
    }

    // Example 3: Real-World High-Frequency Scenario
    println!("3. High-Frequency Telemetry Scenario:");
    {
        // Simulate high-frequency sensor data
        #[derive(Debug)]
        struct SensorData {
            timestamp: u64,
            sensor_id: u32,
            value: f64,
        }

        impl StreamSerialize for SensorData {
            fn serialize(&self, builder: &mut flatbuffers::FlatBufferBuilder) -> Result<()> {
                let data = format!(
                    "ts={},id={},val={:.3}",
                    self.timestamp, self.sensor_id, self.value
                );
                let data_str = builder.create_string(&data);
                builder.finish(data_str, None);
                Ok(())
            }
        }

        // Generate a batch of sensor readings
        let sensor_data: Vec<SensorData> = (0..1000)
            .map(|i| SensorData {
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64
                    + i,
                sensor_id: (i % 10) as u32,
                value: (i as f64) * 0.1,
            })
            .collect();

        // Write batch efficiently
        let file = File::create("sensor_data.bin")?;
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut stream_writer = StreamWriter::new(writer, framer);

        let start = Instant::now();
        stream_writer.write_batch(&sensor_data)?;
        stream_writer.flush()?;
        let write_time = start.elapsed();

        println!(
            "  Wrote {} sensor readings in {:?}",
            sensor_data.len(),
            write_time
        );
        println!(
            "  Throughput: {:.0} messages/second",
            sensor_data.len() as f64 / write_time.as_secs_f64()
        );

        // Read efficiently with zero-allocation
        let file = File::open("sensor_data.bin")?;
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let mut stream_reader = StreamReader::new(reader, deframer);

        let start = Instant::now();
        let mut count = 0;
        while let Some(payload_slice) = stream_reader.read_message()? {
            // Process the sensor data directly from the slice
            // In a real application, you would deserialize the FlatBuffer here
            count += 1;
        }
        let read_time = start.elapsed();

        println!("  Read {} sensor readings in {:?}", count, read_time);
        println!(
            "  Throughput: {:.0} messages/second",
            count as f64 / read_time.as_secs_f64()
        );
        println!("  ✓ High-frequency scenario completed\n");
    }

    println!("=== Performance Example Complete ===");
    println!("Key optimizations demonstrated:");
    println!("  - Write batching: Reduces function call overhead");
    println!("  - Zero-allocation reading: Eliminates per-message heap allocations");
    println!("  - Both optimizations maintain API consistency and safety");

    Ok(())
}
