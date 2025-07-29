use flatstream_rs::{DefaultDeframer, DefaultFramer, StreamReader, StreamSerialize, StreamWriter};
use flatbuffers::FlatBufferBuilder;
use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::time::{SystemTime, UNIX_EPOCH};

fn get_current_timestamp_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

// Define a telemetry event type that implements StreamSerialize
struct TelemetryEvent {
    timestamp: u64,
    device_id: String,
    speed_kph: f32,
    rpm: u32,
    temperature_celsius: f32,
    battery_level: f32,
}

impl StreamSerialize for TelemetryEvent {
    fn serialize(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder,
    ) -> Result<(), flatstream_rs::Error> {
        // Create a simple string representation of the telemetry data
        let telemetry_data = format!(
            "timestamp={},device_id={},speed_kph={:.2},rpm={},temp_c={:.2},battery={:.2}",
            self.timestamp,
            self.device_id,
            self.speed_kph,
            self.rpm,
            self.temperature_celsius,
            self.battery_level
        );

        let data = builder.create_string(&telemetry_data);
        builder.finish(data, None);
        Ok(())
    }
}

fn create_telemetry_event() -> TelemetryEvent {
    let timestamp = get_current_timestamp_nanos();
    let device_id = format!("device-{}", (timestamp % 1000) / 100);
    let speed_kph = (timestamp % 200) as f32 * 0.5; // 0-100 km/h
    let rpm = 800 + ((timestamp % 5000) as u32); // 800-5800 RPM
    let temperature_celsius = 20.0 + ((timestamp % 40) as f32); // 20-60°C
    let battery_level = 100.0 - ((timestamp % 100) as f32); // 0-100%

    TelemetryEvent {
        timestamp,
        device_id,
        speed_kph,
        rpm,
        temperature_celsius,
        battery_level,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let telemetry_file = "expert_telemetry_stream.bin";

    println!("=== Expert Processing Example (v2.5) ===");
    println!("Writing telemetry events to: {}", telemetry_file);

    // Create the telemetry stream file
    let file = File::create(telemetry_file)?;
    let writer = BufWriter::new(file);

    // Create a StreamWriter with default framing
    let framer = DefaultFramer;
    let mut stream_writer = StreamWriter::new(writer, framer);
    
    // External builder management for zero-allocation writes
    let mut builder = FlatBufferBuilder::new();

    // Generate test data
    println!("Generating test telemetry events...");
    let mut event_count = 0;
    
    for _ in 0..100 {
        let event = create_telemetry_event();
        
        // Build message with external builder (zero-allocation)
        builder.reset();
        event.serialize(&mut builder)?;
        
        // Emit to stream (zero-allocation write)
        stream_writer.write(&mut builder)?;
        event_count += 1;
    }

    // Ensure all data is written to disk
    stream_writer.flush()?;
    println!("Generated {} telemetry events", event_count);

    // Now demonstrate expert-level processing with messages()
    println!("\n=== Expert Processing with messages() ===");

    let file = File::open(telemetry_file)?;
    let reader = BufReader::new(file);

    // Create a StreamReader with the same framing strategy used for writing
    let deframer = DefaultDeframer;
    let mut stream_reader = StreamReader::new(reader, deframer);
    
    // Create the messages processor
    let mut messages = stream_reader.messages();

    // Example 1: Chunked Processing
    println!("1. Chunked Processing:");
    let mut chunk = Vec::new();
    let mut chunk_count = 0;
    
    while let Some(payload) = messages.next()? {
        // Convert to owned data for chunking (if needed)
        chunk.push(payload.to_vec());
        
        if chunk.len() >= 10 {
            println!("  Processing chunk {} with {} messages", chunk_count, chunk.len());
            // In a real application, you would process the chunk here
            // For example: send to database, analyze, etc.
            chunk.clear();
            chunk_count += 1;
        }
    }
    
    // Process remaining messages in the last chunk
    if !chunk.is_empty() {
        println!("  Processing final chunk {} with {} messages", chunk_count, chunk.len());
    }

    // Example 2: Early Exit Processing
    println!("\n2. Early Exit Processing:");
    let file = File::open(telemetry_file)?;
    let reader = BufReader::new(file);
    let deframer = DefaultDeframer;
    let mut stream_reader = StreamReader::new(reader, deframer);
    let mut messages = stream_reader.messages();
    
    let mut processed_count = 0;
    let max_events = 25; // Stop after processing 25 events
    
    while let Some(payload) = messages.next()? {
        // Process the payload directly (zero-copy)
        processed_count += 1;
        
        if processed_count % 5 == 0 {
            println!("  Processed {} events...", processed_count);
        }
        
        // Early exit condition
        if processed_count >= max_events {
            println!("  Reached maximum event count ({}), stopping early", max_events);
            break;
        }
    }
    
    println!("  Total events processed: {}", processed_count);

    // Example 3: Conditional Processing
    println!("\n3. Conditional Processing:");
    let file = File::open(telemetry_file)?;
    let reader = BufReader::new(file);
    let deframer = DefaultDeframer;
    let mut stream_reader = StreamReader::new(reader, deframer);
    let mut messages = stream_reader.messages();
    
    let mut high_temp_count = 0;
    let mut normal_temp_count = 0;
    
    while let Some(payload) = messages.next()? {
        // In a real application, you would deserialize the FlatBuffer here
        // For this example, we'll simulate conditional processing based on payload size
        if payload.len() > 100 {
            high_temp_count += 1;
            if high_temp_count % 5 == 0 {
                println!("  High-temperature events: {}", high_temp_count);
            }
        } else {
            normal_temp_count += 1;
        }
    }
    
    println!("  High-temperature events: {}", high_temp_count);
    println!("  Normal-temperature events: {}", normal_temp_count);

    println!("\n=== Expert Processing Complete ===");
    println!("v2.5 Expert API Benefits:");
    println!("  - User-controlled processing loops");
    println!("  - Early exit capabilities");
    println!("  - Chunked processing support");
    println!("  - Conditional processing logic");
    println!("  - Zero-copy slice access");

    Ok(())
} 