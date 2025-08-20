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
    let telemetry_file = "telemetry_stream.bin";

    println!("=== Telemetry Agent Example (v2.5) ===");
    println!("Writing telemetry events to: {telemetry_file}");

    // Create the telemetry stream file
    let file = File::create(telemetry_file)?;
    let writer = BufWriter::new(file);

    // Create a StreamWriter with default framing (no checksums for simplicity)
    let framer = DefaultFramer;
    let mut stream_writer = StreamWriter::new(writer, framer);

    // External builder management for zero-allocation writes
    let mut builder = FlatBufferBuilder::new();

    // Simulate capturing telemetry events for 10 seconds
    println!("Capturing telemetry events...");
    let start_time = SystemTime::now();
    let mut event_count = 0;

    while SystemTime::now().duration_since(start_time)?.as_secs() < 10 {
        // Sample data in real-time (simulate shared memory access)
        let event = create_telemetry_event();

        // Build message with external builder (zero-allocation)
        builder.reset();
        event.serialize(&mut builder)?;

        // Emit to stream (zero-allocation write)
        stream_writer.write_finished(&mut builder)?;
        event_count += 1;

        // Simulate some processing time
        std::thread::sleep(std::time::Duration::from_millis(100));

        if event_count % 10 == 0 {
            println!("  Captured {event_count} events...");
        }
    }

    // Ensure all data is written to disk
    stream_writer.flush()?;
    println!("Captured {event_count} telemetry events");

    // Now demonstrate real-time processing of the telemetry stream
    println!("\n=== Real-time Telemetry Processing ===");
    println!("Processing telemetry events with processor API...");

    let file = File::open(telemetry_file)?;
    let reader = BufReader::new(file);
    let deframer = DefaultDeframer;
    let mut stream_reader = StreamReader::new(reader, deframer);

    let mut processed_count = 0;
    let mut total_speed = 0.0;
    let mut total_rpm = 0.0;
    let mut total_temp = 0.0;
    let mut total_battery = 0.0;
    let mut alerts = 0;

    // Process all telemetry events with zero-allocation
    stream_reader.process_all(|payload| {
        if let Ok(data_str) = std::str::from_utf8(payload) {
            processed_count += 1;

            // Parse telemetry data (in a real system, you'd deserialize the FlatBuffer)
            for part in data_str.split(',') {
                if part.starts_with("speed_kph=") {
                    if let Ok(speed) = part.split('=').nth(1).unwrap_or("0").parse::<f32>() {
                        total_speed += speed;

                        // Alert for high speed
                        if speed > 80.0 {
                            alerts += 1;
                            println!("  ⚠️  High speed alert: {speed:.1} km/h");
                        }
                    }
                } else if part.starts_with("rpm=") {
                    if let Ok(rpm) = part.split('=').nth(1).unwrap_or("0").parse::<f32>() {
                        total_rpm += rpm;

                        // Alert for high RPM
                        if rpm > 5000.0 {
                            alerts += 1;
                            println!("  ⚠️  High RPM alert: {rpm:.0} RPM");
                        }
                    }
                } else if part.starts_with("temp_c=") {
                    if let Ok(temp) = part.split('=').nth(1).unwrap_or("0").parse::<f32>() {
                        total_temp += temp;

                        // Alert for high temperature
                        if temp > 50.0 {
                            alerts += 1;
                            println!("  ⚠️  High temperature alert: {temp:.1}°C");
                        }
                    }
                } else if part.starts_with("battery=") {
                    if let Ok(battery) = part.split('=').nth(1).unwrap_or("0").parse::<f32>() {
                        total_battery += battery;

                        // Alert for low battery
                        if battery < 20.0 {
                            alerts += 1;
                            println!("  ⚠️  Low battery alert: {battery:.1}%");
                        }
                    }
                }
            }

            // Process every 10th event for demonstration
            if processed_count % 10 == 0 {
                println!(
                    "  Processed {processed_count} events, {alerts} alerts so far"
                );
            }
        }
        Ok(())
    })?;

    // Print final statistics
    println!("\n=== Telemetry Analysis Complete ===");
    println!("Total events processed: {processed_count}");
    if processed_count > 0 {
        println!(
            "Average speed: {:.1} km/h",
            total_speed / processed_count as f32
        );
        println!("Average RPM: {:.0}", total_rpm / processed_count as f32);
        println!(
            "Average temperature: {:.1}°C",
            total_temp / processed_count as f32
        );
        println!(
            "Average battery: {:.1}%",
            total_battery / processed_count as f32
        );
    }
    println!("Total alerts generated: {alerts}");

    println!("\n=== Telemetry Agent Example Complete ===");
    println!("Key v2.5 features demonstrated:");
    println!("  • External builder management for zero-allocation writes");
    println!("  • Processor API for high-performance bulk processing");
    println!("  • Real-time telemetry processing with zero-copy access");
    println!("  • Efficient memory usage throughout the pipeline");

    Ok(())
}
