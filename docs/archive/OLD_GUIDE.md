### 1\. Writing a Stream to a File

This example demonstrates how to write a stream of `TelemetryEvent` FlatBuffers messages to a file, with optional checksumming.

```rust
use std::fs::File;
use std::io::BufWriter;
use flatbuffers::FlatBufferBuilder;
use flatstream_rs::{StreamWriter, ChecksumType};

// Assuming telemetry_generated.rs contains the FlatBuffers generated code
#[allow(dead_code, unused_imports)]
#[path = "./telemetry_generated.rs"]
mod telemetry_generated;
pub use telemetry_generated::telemetry::{TelemetryEvent, TelemetryEventArgs};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create("telemetry_stream.bin")?;
    let writer = BufWriter::new(file);

    // Create a StreamWriter with XXH3_64 checksums enabled
    let mut stream_writer = StreamWriter::new(writer, ChecksumType::XxHash64);

    let mut builder = FlatBufferBuilder::new();

    for i in 0..100 {
        let device_id = builder.create_string(&format!("device-{}", i % 5));
        let args = TelemetryEventArgs {
            timestamp_nanos: Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos() as u64),
            device_id: Some(device_id),
            speed_kph: i as f32 * 1.5,
            rpm: 1000 + (i * 100) as u32,
            ..Default::default()
        };
        let event = TelemetryEvent::create(&mut builder, &args);

        // Write the FlatBuffers message to the stream
        stream_writer.write_message(&mut builder, event)?;
    }

    stream_writer.flush()?; // Ensure all buffered data is written to disk
    println!("Successfully wrote 100 telemetry events to telemetry_stream.bin");

    Ok(())
}
```

### 2\. Reading a Stream from a File

This example shows how to read the `TelemetryEvent` messages back from the file, verifying checksums if they were enabled during writing.

```rust
use std::fs::File;
use std::io::BufReader;
use flatstream_rs::{StreamReader, ChecksumType};
use flatbuffers::get_root;

// Assuming telemetry_generated.rs contains the FlatBuffers generated code
#[allow(dead_code, unused_imports)]
#[path = "./telemetry_generated.rs"]
mod telemetry_generated;
pub use telemetry_generated::telemetry::TelemetryEvent;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file = File::open("telemetry_stream.bin")?;
    let reader = BufReader::new(file);

    // Create a StreamReader, specifying the checksum type used during writing
    let stream_reader = StreamReader::new(reader, ChecksumType::XxHash64);

    println!("Reading telemetry events from telemetry_stream.bin:");
    let mut count = 0;
    for result in stream_reader {
        match result {
            Ok(payload) => {
                // Get zero-copy access to the FlatBuffers root
                let event = get_root::<TelemetryEvent>(&payload)?;
                println!(
                    "  Event {}: Device ID: {}, Speed: {:.2} kph, RPM: {}",
                    count,
                    event.device_id().unwrap_or("N/A"),
                    event.speed_kph(),
                    event.rpm()
                );
                count += 1;
            }
            Err(e) => {
                eprintln!("Error reading stream: {}", e);
                break;
            }
        }
    }
    println!("Finished reading {} telemetry events.", count);

    Ok(())
}
```

### 3\. Disabling Checksums

If data integrity is handled by a lower layer (e.g., TCP checksums, file system integrity), you can disable checksumming for maximum raw throughput:

```rust
// When creating the writer
let mut stream_writer = StreamWriter::new(writer, ChecksumType::None);

// When creating the reader
let stream_reader = StreamReader::new(reader, ChecksumType::None);
```

## Use Case: Telemetry Capturing Agent

This library is ideally suited for a telemetry capturing agent process that needs to emit long-running streams of data (up to 24 hours). The agent can continuously write FlatBuffers messages to a local file using `flatstream-rs`. This file then serves as a durable record for later reprocessing, analysis, or transfer. The optional checksums provide an essential layer of protection against data corruption during capture and storage.

-----

**Note on Timestamp Accuracy:** The FlatBuffers schema uses `u64` for `timestamp_nanos`, allowing for nanosecond precision, which fully supports the sub-millisecond granularity requirement. The `std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos() as u64` conversion ensures this precision is captured.
