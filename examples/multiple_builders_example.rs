//! Example demonstrating the use of multiple builders in expert mode for different message types.
//!
//! This pattern is particularly useful when your application handles messages of
//! vastly different sizes, preventing memory waste from builder bloat.

use flatbuffers::FlatBufferBuilder;
use flatstream_rs::*;
use std::fs::File;
use std::io::BufWriter;

// Small, frequent control messages
struct ControlMessage {
    command: String,
    #[allow(dead_code)]
    timestamp: u64,
}

impl StreamSerialize for ControlMessage {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let cmd = builder.create_string(&self.command);
        builder.finish(cmd, None);
        Ok(())
    }
}

// Medium-sized telemetry batches
struct TelemetryBatch {
    device_id: String,
    readings: Vec<f64>,
}

impl StreamSerialize for TelemetryBatch {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let _id = builder.create_string(&self.device_id);
        let readings = builder.create_vector(&self.readings);
        builder.finish(readings, None);
        Ok(())
    }
}

// Large file transfer chunks
struct FileChunk {
    file_id: String,
    #[allow(dead_code)]
    chunk_number: u32,
    data: Vec<u8>,
}

impl StreamSerialize for FileChunk {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let _id = builder.create_string(&self.file_id);
        let data = builder.create_vector(&self.data);
        builder.finish(data, None);
        Ok(())
    }
}

enum Message {
    Control(ControlMessage),
    Telemetry(TelemetryBatch),
    FileTransfer(FileChunk),
}

fn main() -> Result<()> {
    println!("=== Multiple Builders Example ===\n");

    // Create output file
    let file = File::create("multi_message_stream.bin")?;
    let writer = BufWriter::new(file);
    let mut stream_writer = StreamWriter::new(writer, DefaultFramer);

    // Create separate builders for each message type
    // This prevents small messages from being serialized in a builder
    // that has grown to accommodate large file transfers
    let mut control_builder = FlatBufferBuilder::new();
    let mut telemetry_builder = FlatBufferBuilder::new();
    let mut file_builder = FlatBufferBuilder::new();

    // Simulate a mixed workload
    let messages = vec![
        // Small control messages
        Message::Control(ControlMessage {
            command: "START".to_string(),
            timestamp: 1000,
        }),
        Message::Control(ControlMessage {
            command: "SET_RATE=100".to_string(),
            timestamp: 1001,
        }),
        
        // Medium telemetry batch
        Message::Telemetry(TelemetryBatch {
            device_id: "sensor-001".to_string(),
            readings: vec![23.5, 24.1, 23.8, 24.0, 23.9],
        }),
        
        // Large file chunk (1MB)
        Message::FileTransfer(FileChunk {
            file_id: "firmware-v2.0.bin".to_string(),
            chunk_number: 1,
            data: vec![0xAB; 1024 * 1024], // 1MB chunk
        }),
        
        // More control messages after the large transfer
        Message::Control(ControlMessage {
            command: "STATUS".to_string(),
            timestamp: 2000,
        }),
        Message::Control(ControlMessage {
            command: "STOP".to_string(),
            timestamp: 2001,
        }),
    ];

    // Process messages using the appropriate builder for each type
    for (i, message) in messages.iter().enumerate() {
        match message {
            Message::Control(msg) => {
                println!("Writing control message #{}", i);
                control_builder.reset();
                msg.serialize(&mut control_builder)?;
                stream_writer.write_finished(&mut control_builder)?;
            }
            Message::Telemetry(msg) => {
                println!("Writing telemetry batch #{}", i);
                telemetry_builder.reset();
                msg.serialize(&mut telemetry_builder)?;
                stream_writer.write_finished(&mut telemetry_builder)?;
            }
            Message::FileTransfer(msg) => {
                println!("Writing file chunk #{} (1MB)", i);
                file_builder.reset();
                msg.serialize(&mut file_builder)?;
                stream_writer.write_finished(&mut file_builder)?;
                
                // Optional: For very rare large messages, you could even drop
                // and recreate the builder to free memory immediately
                // file_builder = FlatBufferBuilder::new();
            }
        }
    }

    stream_writer.flush()?;
    println!("\n✅ Messages written successfully!");

    // Memory efficiency analysis
    println!("\nMemory Efficiency:");
    println!("- Control messages used a small builder (~1KB capacity)");
    println!("- Telemetry messages used a medium builder (~10KB capacity)");
    println!("- File transfer used a large builder (~1MB capacity)");
    println!("\nWithout multiple builders, ALL messages would use 1MB+ after the file transfer!");

    Ok(())
} 