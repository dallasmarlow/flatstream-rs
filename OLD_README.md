flatstream: Durable, High-Performance FlatBuffers Streaming(https://docs.rs/flatstream/badge.svg)](https://docs.rs/flatstream)(https://github.com/your-username/flatstream/workflows/CI/badge.svg)](https://github.com/your-username/flatstream/actions)(https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)flatstream is a Rust library for writing and reading sequences of FlatBuffers messages to a durable, replayable stream format. It is designed for high-performance telemetry and event-sourcing use cases where long-running streams of data must be captured with minimal overhead and replayed efficiently.The library provides a simple, ergonomic API for appending messages to a stream and iterating over them later, with zero-copy access to the underlying data. It also features optional, pluggable checksums to ensure data integrity against corruption from hardware failures or network issues.FeaturesHigh-Performance Streaming: Appends FlatBuffers messages to any std::io::Write sink with low overhead.Zero-Copy Reading: Iterates through message streams from any std::io::Read source without deserializing or copying message data.Durable & Simple Format: The file format is designed to be simple, robust, and suitable for long-term storage of streams up to 24 hours or more.Data Integrity: Optional, pluggable checksums (CRC32c, xxHash64) protect each message against corruption. Checksumming can be disabled for maximum performance.Ergonomic API: Provides a simple Writer and an idiomatic Reader iterator, abstracting away the complexities of size-prefixing, buffer management, and verification.File FormatThe flatstream format is designed for simplicity and robustness. It consists of a header followed by a sequence of message blocks.[ 4-byte u32: Header Length                                    ][ 4-byte u32: Checksum for Message 1 (optional)                ][ 4-byte u32: Length of Message 1                              ][ 4-byte u32: Checksum for Message 2 (optional)                ][ 4-byte u32: Length of Message 2                              ]...The File Header is a self-describing FlatBuffer message that contains metadata about the stream, such as the version and the checksum algorithm used, allowing the reader to process the file without prior configuration.Usage1. Add DependenciesAdd flatstream to your Cargo.toml. You also need flatbuffers and any checksum crates you intend to use.Ini, TOML[dependencies]
flatbuffers = "24.3"
flatstream = { version = "0.1.0", features = ["crc", "xxhash"] }

# Add your checksum dependencies
crc32fast = { version = "1.4", optional = true }
xxhash-rust = { version = "0.8", features = ["xxh3"], optional = true }

[build-dependencies]
flatbuffers-build = "0.2"
2. Define a SchemaCreate your FlatBuffers schema. For this example, we'll use a simple telemetry event schema.schemas/telemetry.fbs:Code snippetnamespace Telemetry;

table TelemetryEvent {
  // Nanoseconds since UNIX epoch
  timestamp_ns: ulong (required);
  source_id: string;
  value: double;
}

root_type TelemetryEvent;
3. Writing a StreamThe Writer abstracts away all the details of framing, checksumming, and writing the data. You can configure it with a WriteOptions builder.Rustuse flatstream::{Writer, WriteOptions, ChecksumType};
use std::fs::File;
use std::time::SystemTime;

// Include the generated code from your build.rs
mod generated;
use generated::telemetry_generated::telemetry::{
    TelemetryEvent, TelemetryEventArgs,
};

fn main() -> std::io::Result<()> {
    let file = File::create("telemetry.fbs")?;
    let options = WriteOptions::new().checksum(ChecksumType::Crc32c);
    let mut writer = Writer::new(file, options)?;

    let mut builder = flatbuffers::FlatBufferBuilder::new();

    // Write 100 events to the stream
    for i in 0..100 {
        builder.reset();

        let source_id = builder.create_string("sensor-alpha");
        let args = TelemetryEventArgs {
            timestamp_ns: SystemTime::now()
               .duration_since(SystemTime::UNIX_EPOCH)
               .unwrap()
               .as_nanos() as u64,
            source_id: Some(source_id),
            value: i as f64,
        };
        let event = TelemetryEvent::create(&mut builder, &args);
        builder.finish(event, None);

        writer.append(builder.finished_data())?;
    }

    writer.finish()?;
    Ok(())
}
4. Reading a StreamThe Reader provides an iterator over the messages in the stream. It automatically reads the header, validates checksums, and provides zero-copy access to each message.Rustuse flatstream::Reader;
use std::fs::File;

// Include the generated code
mod generated;
use generated::telemetry_generated::telemetry::{
    root_as_telemetry_event, TelemetryEvent,
};

fn main() -> std::io::Result<()> {
    let file = File::open("telemetry.fbs")?;
    let reader = Reader::new(file)?;

    println!(
        "Reading from stream with checksum type: {:?}",
        reader.header().checksum()
    );

    for message_result in reader {
        match message_result {
            Ok(message_bytes) => {
                // Get zero-copy access to the message
                let event = root_as_telemetry_event(&message_bytes).unwrap();
                println!(
                    "Read event: timestamp={}, source='{}', value={}",
                    event.timestamp_ns(),
                    event.source_id().unwrap_or("N/A"),
                    event.value()
                );
            }
            Err(e) => {
                eprintln!("Error reading message: {}", e);
            }
        }
    }

    Ok(())
}
5. Configuring ChecksumsYou can easily configure the checksum algorithm or disable it entirely. This is ideal for balancing data integrity with maximum performance.Rustuse flatstream::{WriteOptions, ChecksumType};

// Use CRC32c (fast and reliable for detecting corruption)
let options_crc = WriteOptions::new().checksum(ChecksumType::Crc32c);

// Use xxHash64 (extremely fast)
let options_xxhash = WriteOptions::new().checksum(ChecksumType::Xxh64);

// Disable checksums for performance-critical scenarios where integrity is handled elsewhere
let options_none = WriteOptions::new().checksum(ChecksumType::None);
The Reader will automatically detect and verify the checksum used when the file was written.LicenseThis project is licensed under either ofApache License, Version 2.0, (LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0)MIT license (LICENSE-MIT or http://opensource.org/licenses/MIT)at your option.ContributionUnless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
