use flatstream_rs::{DefaultDeframer, DefaultFramer, StreamReader, StreamSerialize, StreamWriter};
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
    let telemetry_file = "telemetry_stream.bin";

    println!("=== Telemetry Agent Example ===");
    println!("Writing telemetry events to: {}", telemetry_file);

    // Create the telemetry stream file
    let file = File::create(telemetry_file)?;
    let writer = BufWriter::new(file);

    // Create a StreamWriter with default framing (no checksums for simplicity)
    let framer = DefaultFramer;
    let mut stream_writer = StreamWriter::new(writer, framer);

    // Simulate capturing telemetry events for 10 seconds
    println!("Capturing telemetry events...");
    let start_time = SystemTime::now();
    let mut event_count = 0;

    while SystemTime::now().duration_since(start_time)?.as_secs() < 10 {
        // Create telemetry event
        let event = create_telemetry_event();

        // Write to stream
        stream_writer.write(&event)?;
        event_count += 1;

        // Simulate some processing time
        std::thread::sleep(std::time::Duration::from_millis(100));

        if event_count % 10 == 0 {
            println!("  Captured {} events...", event_count);
        }
    }

    // Ensure all data is written to disk
    stream_writer.flush()?;
    println!("Finished capturing {} telemetry events", event_count);

    // Now demonstrate reading the telemetry stream back
    println!("\n=== Reading Telemetry Stream ===");

    let file = File::open(telemetry_file)?;
    let reader = BufReader::new(file);

    // Create a StreamReader with the same framing strategy used for writing
    let deframer = DefaultDeframer;
    let stream_reader = StreamReader::new(reader, deframer);

    let mut read_count = 0;

    for result in stream_reader {
        match result {
            Ok(_payload) => {
                // In a real application, you would deserialize the FlatBuffer here
                // For this example, we'll just count the events
                read_count += 1;

                // Simulate some processing of the telemetry data
                if read_count % 20 == 0 {
                    println!("  Processed {} events...", read_count);
                }
            }
            Err(e) => {
                eprintln!("Error reading telemetry stream: {}", e);
                break;
            }
        }
    }

    println!(
        "Successfully read {} telemetry events from stream",
        read_count
    );

    // Verify data integrity
    if event_count == read_count {
        println!("✓ Data integrity verified: all events captured and read successfully");
    } else {
        println!(
            "✗ Data integrity issue: captured {} events, read {} events",
            event_count, read_count
        );
    }

    println!("\n=== Example Complete ===");
    println!("Telemetry stream file: {}", telemetry_file);
    println!(
        "File size: {} bytes",
        std::fs::metadata(telemetry_file)?.len()
    );

    Ok(())
}
