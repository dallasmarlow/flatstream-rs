use flatstream_rs::*;
use std::fs::File;
use std::io::{BufReader, BufWriter};

// Example 1: Simple data structure that implements StreamSerialize
#[derive(Debug)]
struct SensorReading {
    sensor_id: String,
    temperature: f32,
    humidity: f32,
    timestamp: u64,
}

impl StreamSerialize for SensorReading {
    fn serialize(&self, builder: &mut flatbuffers::FlatBufferBuilder) -> Result<()> {
        // Create a structured representation of the sensor data
        let sensor_id = builder.create_string(&self.sensor_id);
        let data = format!(
            "temp={:.2},humidity={:.2},ts={}",
            self.temperature, self.humidity, self.timestamp
        );
        let reading_data = builder.create_string(&data);

        // For this example, we'll just use the reading data as the root
        builder.finish(reading_data, None);
        Ok(())
    }
}

// Example 2: More complex data structure
#[derive(Debug)]
struct SystemEvent {
    event_type: String,
    severity: u8,
    message: String,
    metadata: std::collections::HashMap<String, String>,
}

impl StreamSerialize for SystemEvent {
    fn serialize(&self, builder: &mut flatbuffers::FlatBufferBuilder) -> Result<()> {
        // Create a structured representation
        let event_type = builder.create_string(&self.event_type);
        let message = builder.create_string(&self.message);

        // Convert metadata to a string representation
        let metadata_str = self
            .metadata
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect::<Vec<_>>()
            .join(",");
        let metadata = builder.create_string(&metadata_str);

        // Combine all fields into a single string for simplicity
        let data = format!(
            "type={},severity={},msg={},meta={}",
            self.event_type, self.severity, self.message, metadata_str
        );
        let event_data = builder.create_string(&data);

        builder.finish(event_data, None);
        Ok(())
    }
}

fn main() -> Result<()> {
    println!("=== Composable flatstream-rs Example ===\n");

    // Example 1: Basic usage with default framing
    println!("1. Basic usage with default framing:");
    {
        let file = File::create("sensor_data.bin")?;
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut stream_writer = StreamWriter::new(writer, framer);

        // Write some sensor readings
        for i in 0..5 {
            let reading = SensorReading {
                sensor_id: format!("sensor-{}", i),
                temperature: 20.0 + (i as f32 * 2.5),
                humidity: 45.0 + (i as f32 * 5.0),
                timestamp: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            };

            stream_writer.write(&reading)?;
            println!("  Wrote sensor reading: {:?}", reading);
        }

        stream_writer.flush()?;
        println!("  ✓ Wrote 5 sensor readings to sensor_data.bin\n");
    }

    // Read back the sensor data
    {
        let file = File::open("sensor_data.bin")?;
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let stream_reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        for result in stream_reader {
            let payload = result?;
            println!("  Read sensor data: {} bytes", payload.len());
            count += 1;
        }
        println!("  ✓ Read {} sensor readings back\n", count);
    }

    // Example 2: Using checksums (if feature is enabled)
    #[cfg(feature = "checksum")]
    {
        println!("2. Using checksums for data integrity:");
        let file = File::create("system_events.bin")?;
        let writer = BufWriter::new(file);
        let checksum = XxHash64::new();
        let framer = ChecksumFramer::new(checksum);
        let mut stream_writer = StreamWriter::new(writer, framer);

        // Write some system events
        for i in 0..3 {
            let mut metadata = std::collections::HashMap::new();
            metadata.insert("user_id".to_string(), format!("user-{}", i));
            metadata.insert("session_id".to_string(), format!("session-{}", i * 100));

            let event = SystemEvent {
                event_type: "INFO".to_string(),
                severity: 1,
                message: format!("System event number {}", i),
                metadata,
            };

            stream_writer.write(&event)?;
            println!("  Wrote system event: {:?}", event);
        }

        stream_writer.flush()?;
        println!("  ✓ Wrote 3 system events with checksums to system_events.bin\n");

        // Read back with checksum verification
        let file = File::open("system_events.bin")?;
        let reader = BufReader::new(file);
        let checksum = XxHash64::new();
        let deframer = ChecksumDeframer::new(checksum);
        let stream_reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        for result in stream_reader {
            let payload = result?;
            println!(
                "  Read system event: {} bytes (checksum verified)",
                payload.len()
            );
            count += 1;
        }
        println!(
            "  ✓ Read {} system events with checksum verification\n",
            count
        );
    }

    #[cfg(not(feature = "checksum"))]
    {
        println!("2. Checksum feature not enabled - skipping checksum example");
        println!("   To enable, run: cargo run --example composable_example --features checksum\n");
    }

    // Example 3: Demonstrating the composability
    println!("3. Demonstrating composability:");
    println!("   - StreamSerialize trait allows any type to be serialized");
    println!("   - Framer/Deframer traits allow custom framing strategies");
    println!("   - Checksum trait allows pluggable integrity checking");
    println!("   - All components work together through trait composition\n");

    // Example 4: Using built-in string serialization
    println!("4. Using built-in string serialization:");
    {
        let file = File::create("string_data.bin")?;
        let writer = BufWriter::new(file);
        let framer = DefaultFramer;
        let mut stream_writer = StreamWriter::new(writer, framer);

        let messages = vec![
            "Hello, world!",
            "This is a test message",
            "FlatBuffers streaming is awesome",
        ];

        for message in &messages {
            stream_writer.write(message)?;
            println!("  Wrote string: '{}'", message);
        }

        stream_writer.flush()?;
        println!("  ✓ Wrote {} string messages\n", messages.len());

        // Read back
        let file = File::open("string_data.bin")?;
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let stream_reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        for result in stream_reader {
            let payload = result?;
            println!("  Read string data: {} bytes", payload.len());
            count += 1;
        }
        println!("  ✓ Read {} string messages back\n", count);
    }

    println!("=== Example Complete ===");
    println!("Files created:");
    println!("  - sensor_data.bin");
    println!("  - string_data.bin");
    #[cfg(feature = "checksum")]
    println!("  - system_events.bin");

    Ok(())
}
