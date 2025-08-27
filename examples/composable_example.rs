// Example purpose: Compose framers/deframers with user-defined serializable types.
// Shows default vs checksum framing and end-to-end processing patterns.
use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::fs::File;
use std::io::{BufReader, BufWriter};

// Example 1: Simple data structure that implements StreamSerialize
#[derive(Debug)]
struct SensorReading {
    sensor_id: String,
    temperature: f32,
    #[allow(dead_code)]
    humidity: f32,
    timestamp: u64,
}

impl StreamSerialize for SensorReading {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let data = format!(
            "{},{},{},{}",
            &self.sensor_id, self.timestamp, self.temperature, "C"
        );
        let data_str = builder.create_string(&data);
        builder.finish(data_str, None);
        Ok(())
    }
}

// Example 2: More complex data structure
#[derive(Debug)]
#[allow(dead_code)]
struct SystemEvent {
    event_type: String,
    severity: u8,
    message: String,
    metadata: std::collections::HashMap<String, String>,
}

impl StreamSerialize for SystemEvent {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let data = format!(
            "{},{},{},{}",
            &self.event_type, "timestamp", self.severity, &self.message
        );
        let data_str = builder.create_string(&data);
        builder.finish(data_str, None);
        Ok(())
    }
}

fn main() -> Result<()> {
    println!("=== Composable flatstream-rs v2.5 Example ===\n");

    // Example 1: Basic usage with default framing
    println!("1. Basic usage with default framing:");
    {
        let file = File::create("sensor_data.bin")?;
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut stream_writer = StreamWriter::new(writer, framer);

        // External builder management for zero-allocation writes
        let mut builder = FlatBufferBuilder::new();

        // Write some sensor readings
        for i in 0..5 {
            let reading = SensorReading {
                sensor_id: format!("sensor-{i}"),
                temperature: 20.0 + (i as f32 * 2.5),
                humidity: 45.0 + (i as f32 * 5.0),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            };

            // Build and write with external builder
            builder.reset();
            reading.serialize(&mut builder)?;
            stream_writer.write_finished(&mut builder)?;
            println!("  Wrote sensor reading: {reading:?}");
        }

        stream_writer.flush()?;
        println!("  ✓ Wrote 5 sensor readings to sensor_data.bin\n");
    }

    // Read back the sensor data using the processor API
    {
        let file = File::open("sensor_data.bin")?;
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let mut stream_reader = StreamReader::new(reader, deframer);

        println!("  Reading sensor data back:");
        let mut count = 0;
        stream_reader.process_all(|payload| {
            if let Ok(data_str) = std::str::from_utf8(payload) {
                println!("    Message {}: {}", count + 1, data_str);
                count += 1;
            }
            Ok(())
        })?;
        println!("  ✓ Read {count} sensor readings\n");
    }

    // Example 2: Using checksum framing for data integrity
    #[cfg(feature = "xxhash")]
    {
        println!("2. Using checksum framing for data integrity:");
        let file = File::create("secure_sensor_data.bin")?;
        let writer = BufWriter::new(file);
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut stream_writer = StreamWriter::new(writer, framer);

        // External builder management
        let mut builder = FlatBufferBuilder::new();

        // Write some sensor readings with checksum protection
        for i in 0..3 {
            let reading = SensorReading {
                sensor_id: format!("secure-sensor-{i}"),
                temperature: 25.0 + (i as f32 * 1.5),
                humidity: 50.0 + (i as f32 * 3.0),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            };

            builder.reset();
            reading.serialize(&mut builder)?;
            stream_writer.write_finished(&mut builder)?;
            println!("  Wrote secure sensor reading: {reading:?}");
        }

        stream_writer.flush()?;
        println!("  ✓ Wrote 3 secure sensor readings to secure_sensor_data.bin\n");

        // Read back the secure sensor data
        let file = File::open("secure_sensor_data.bin")?;
        let reader = BufReader::new(file);
        let checksum = XxHash64::new();
        let deframer = ChecksumDeframer::new(checksum);
        let mut stream_reader = StreamReader::new(reader, deframer);

        println!("  Reading secure sensor data back:");
        let mut count = 0;
        stream_reader.process_all(|payload| {
            if let Ok(data_str) = std::str::from_utf8(payload) {
                println!("    Secure message {}: {}", count + 1, data_str);
                count += 1;
            }
            Ok(())
        })?;
        println!("  ✓ Read {count} secure sensor readings\n");
    }

    // Example 3: Complex system events
    println!("3. Complex system events:");
    {
        let file = File::create("system_events.bin")?;
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut stream_writer = StreamWriter::new(writer, framer);

        // External builder management
        let mut builder = FlatBufferBuilder::new();

        // Create some system events
        let events = vec![
            SystemEvent {
                event_type: "INFO".to_string(),
                severity: 1,
                message: "System startup complete".to_string(),
                metadata: {
                    let mut map = std::collections::HashMap::new();
                    map.insert("component".to_string(), "core".to_string());
                    map.insert("version".to_string(), "1.0.0".to_string());
                    map
                },
            },
            SystemEvent {
                event_type: "WARNING".to_string(),
                severity: 2,
                message: "High memory usage detected".to_string(),
                metadata: {
                    let mut map = std::collections::HashMap::new();
                    map.insert("memory_usage".to_string(), "85%".to_string());
                    map.insert("threshold".to_string(), "80%".to_string());
                    map
                },
            },
            SystemEvent {
                event_type: "ERROR".to_string(),
                severity: 3,
                message: "Database connection failed".to_string(),
                metadata: {
                    let mut map = std::collections::HashMap::new();
                    map.insert("retry_count".to_string(), "3".to_string());
                    map.insert("timeout".to_string(), "30s".to_string());
                    map
                },
            },
        ];

        // Write system events
        for event in &events {
            builder.reset();
            event.serialize(&mut builder)?;
            stream_writer.write_finished(&mut builder)?;
            println!("  Wrote system event: {event:?}");
        }

        stream_writer.flush()?;
        println!(
            "  ✓ Wrote {} system events to system_events.bin\n",
            events.len()
        );

        // Read back system events using expert API for manual control
        let file = File::open("system_events.bin")?;
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let mut stream_reader = StreamReader::new(reader, deframer);

        println!("  Reading system events back:");
        let mut count = 0;
        let mut messages = stream_reader.messages();
        while let Some(payload) = messages.next()? {
            if let Ok(data_str) = std::str::from_utf8(payload) {
                println!("    System event {}: {}", count + 1, data_str);
                count += 1;
            }
        }
        println!("  ✓ Read {count} system events\n");
    }

    println!("=== Composable Example Complete ===");
    println!("Key v2.5 features demonstrated:");
    println!("  • External builder management for zero-allocation writes");
    println!("  • Processor API for high-performance bulk processing");
    println!("  • Expert API for manual iteration control");
    println!("  • Composable framing strategies");
    println!("  • Zero-copy message processing throughout");

    Ok(())
}
