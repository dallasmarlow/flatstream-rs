//! Example demonstrating the use of multiple builders in expert mode for different message types.
//!
//! This pattern is particularly useful when your application handles messages of
//! vastly different sizes, preventing memory waste from builder bloat.
//! Example purpose: Contrast separate builders per size/type vs a single builder.

use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::io::Cursor;

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

    // In-memory sink: the example is about builder management, not file I/O,
    // and examples should not drop files into the working directory.
    let mut out = Vec::new();
    let mut stream_writer = StreamWriter::new(Cursor::new(&mut out), DefaultFramer);

    // Create separate builders for each message type
    // This prevents small messages from being serialized in a builder
    // that has grown to accommodate large file transfers
    let mut control_builder = FlatBufferBuilder::new();
    let mut telemetry_builder = FlatBufferBuilder::new();
    let mut file_builder = FlatBufferBuilder::new();

    // Simulate a mixed workload
    let messages = [
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
                println!("Writing control message #{i}");
                control_builder.reset();
                msg.serialize(&mut control_builder)?;
                stream_writer.write_finished(&mut control_builder)?;
            }
            Message::Telemetry(msg) => {
                println!("Writing telemetry batch #{i}");
                telemetry_builder.reset();
                msg.serialize(&mut telemetry_builder)?;
                stream_writer.write_finished(&mut telemetry_builder)?;
            }
            Message::FileTransfer(msg) => {
                println!("Writing file chunk #{i} (1MB)");
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
    drop(stream_writer);

    // Prove every heterogeneous payload was framed and remains readable. The
    // example is about builder ownership, not schema decoding, so frame count
    // is the relevant end-to-end assertion here.
    let mut reader = StreamReader::new(Cursor::new(&out), DefaultDeframer::new());
    let mut frame_count = 0usize;
    reader.process_all(|_| {
        frame_count += 1;
        Ok(())
    })?;
    assert_eq!(frame_count, messages.len());

    println!("\n✅ Messages written successfully!");

    // Memory efficiency: this is the actual thesis of the example, so measure
    // it rather than asserting it in prose. A builder never shrinks on
    // `reset()`, so the one that served the 1MB chunk stays 1MB-sized while
    // the control builder — the hot path — stays tiny.
    let control_cap = capacity_of(&mut control_builder);
    let telemetry_cap = capacity_of(&mut telemetry_builder);
    let file_cap = capacity_of(&mut file_builder);

    println!("\nMemory Efficiency (backing buffer after the run):");
    println!("- control_builder:   {control_cap:>9} bytes");
    println!("- telemetry_builder: {telemetry_cap:>9} bytes");
    println!("- file_builder:      {file_cap:>9} bytes");

    assert!(
        file_cap >= 1024 * 1024,
        "the file builder should have grown to hold the 1MB chunk, was {file_cap}"
    );
    assert!(
        control_cap * 64 < file_cap,
        "the whole point of separate builders: the control builder ({control_cap} bytes) \
         must not have been dragged up to the file builder's size ({file_cap} bytes)"
    );
    assert!(
        telemetry_cap < file_cap,
        "the telemetry builder ({telemetry_cap}) should also be unaffected by the 1MB chunk"
    );

    println!(
        "\nThe control builder stayed {}x smaller than the file builder — a single",
        file_cap / control_cap.max(1)
    );
    println!("shared builder would have left every small write on a 1MB buffer.");

    Ok(())
}

/// The builder's backing-buffer size, read the same way `StreamWriter`'s memory
/// policy reads it: `FlatBufferBuilder` exposes no `capacity()` getter, but
/// `mut_finished_buffer()` hands back the backing buffer, whose length is the
/// effective capacity. Valid only on a finished builder.
fn capacity_of(builder: &mut FlatBufferBuilder<'_>) -> usize {
    builder.mut_finished_buffer().0.len()
}
