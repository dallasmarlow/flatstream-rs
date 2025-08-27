// Example purpose: Demonstrates expert-mode reading patterns, including typed vs raw payload
// processing, and how to structure processing pipelines with zero-copy access.
use flatbuffers::FlatBufferBuilder;
use flatstream::{DefaultDeframer, DefaultFramer, StreamReader, StreamSerialize, StreamWriter};
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
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<(), flatstream::Error> {
        let data = format!(
            "{},{},{},{},{},{}",
            &self.device_id,
            self.timestamp,
            self.speed_kph,
            self.rpm,
            self.temperature_celsius,
            self.battery_level
        );
        let data_str = builder.create_string(&data);
        builder.finish(data_str, None);
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
    println!("Writing telemetry events to: {telemetry_file}");

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
        stream_writer.write_finished(&mut builder)?;
        event_count += 1;
    }

    // Ensure all data is written to disk
    stream_writer.flush()?;
    println!("Generated {event_count} telemetry events");

    // Now demonstrate the expert reading patterns
    println!("\n=== Reading Telemetry Data ===");

    // Pattern 1: Processor API (highest performance)
    println!("1. Processor API (Zero-Allocation Processing):");
    {
        let file = File::open(telemetry_file)?;
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let mut stream_reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        let mut total_speed = 0.0;
        let mut total_rpm = 0.0;
        let mut total_temp = 0.0;
        let mut total_battery = 0.0;

        // Process all events with zero-allocation
        stream_reader.process_all(|payload| {
            // In a real application, you would deserialize the FlatBuffer here
            // For this example, we simulate processing by parsing the string
            if let Ok(data_str) = std::str::from_utf8(payload) {
                // Simple parsing for demonstration
                for part in data_str.split(',') {
                    if part.starts_with("speed_kph=") {
                        if let Ok(speed) = part.split('=').nth(1).unwrap_or("0").parse::<f32>() {
                            total_speed += speed;
                        }
                    } else if part.starts_with("rpm=") {
                        if let Ok(rpm) = part.split('=').nth(1).unwrap_or("0").parse::<f32>() {
                            total_rpm += rpm;
                        }
                    } else if part.starts_with("temp_c=") {
                        if let Ok(temp) = part.split('=').nth(1).unwrap_or("0").parse::<f32>() {
                            total_temp += temp;
                        }
                    } else if part.starts_with("battery=") {
                        if let Ok(battery) = part.split('=').nth(1).unwrap_or("0").parse::<f32>() {
                            total_battery += battery;
                        }
                    }
                }
                count += 1;
            }
            Ok(())
        })?;

        println!("  Processed {count} events");
        if count > 0 {
            println!("  Average speed: {:.1} km/h", total_speed / count as f32);
            println!("  Average RPM: {:.0}", total_rpm / count as f32);
            println!("  Average temperature: {:.1}°C", total_temp / count as f32);
            println!("  Average battery: {:.1}%", total_battery / count as f32);
        }
    }

    // Pattern 2: Expert API (manual control)
    println!("\n2. Expert API (Manual Control):");
    {
        let file = File::open(telemetry_file)?;
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let mut stream_reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        let mut alerts = 0;

        // Use expert API for conditional processing
        let mut messages = stream_reader.messages();
        while let Some(payload) = messages.next()? {
            // Process each message with manual control
            if let Ok(data_str) = std::str::from_utf8(payload) {
                count += 1;

                // Check for alert conditions
                if data_str.contains("temp_c=5") || data_str.contains("battery=1") {
                    alerts += 1;
                    println!("  Alert #{alerts}: {data_str}");
                }

                // Stop after processing first 10 events for demonstration
                if count >= 10 {
                    println!("  Stopped after {count} events (demonstration)");
                    break;
                }
            }
        }

        println!("  Processed {count} events, found {alerts} alerts");
    }

    // Pattern 3: Real-time processing simulation
    println!("\n3. Real-time Processing Simulation:");
    {
        let file = File::open(telemetry_file)?;
        let reader = BufReader::new(file);
        let deframer = DefaultDeframer;
        let mut stream_reader = StreamReader::new(reader, deframer);

        let mut count = 0;
        let mut high_speed_events = 0;
        let mut low_battery_events = 0;

        // Simulate real-time processing with immediate decisions
        stream_reader.process_all(|payload| {
            if let Ok(data_str) = std::str::from_utf8(payload) {
                count += 1;

                // Real-time decision making
                if data_str.contains("speed_kph=9") {
                    high_speed_events += 1;
                    // In a real system, this might trigger an alert
                }

                if data_str.contains("battery=1") {
                    low_battery_events += 1;
                    // In a real system, this might trigger power management
                }

                // Process every 10th event for demonstration
                if count % 10 == 0 {
                    println!(
                        "  Processed {count} events (high_speed: {high_speed_events}, low_battery: {low_battery_events})"
                    );
                }
            }
            Ok(())
        })?;

        println!("  Final stats: {count} total events");
        println!("  High speed events: {high_speed_events}");
        println!("  Low battery events: {low_battery_events}");
    }

    println!("\n=== Expert Processing Example Complete ===");
    println!("Key v2.5 features demonstrated:");
    println!("  • External builder management for zero-allocation writes");
    println!("  • Processor API for high-performance bulk processing");
    println!("  • Expert API for manual iteration control");
    println!("  • Zero-copy message processing throughout");

    Ok(())
}
