//! Example demonstrating the Adaptive Memory Policy for the StreamWriter.
//!
//! This example simulates a "bursty" workload where a large message causes
//! buffer expansion, followed by many small messages that eventually trigger
//! a memory reclamation event (resetting the internal builder).

use flatbuffers::FlatBufferBuilder;
use flatstream::policy::{AdaptiveWatermarkPolicy, MemoryPolicy, ReclamationInfo, ReclamationReason};
use flatstream::{DefaultFramer, StreamSerialize, StreamWriter};

// A simple serializable wrapper for byte vectors
struct Blob(Vec<u8>);

impl StreamSerialize for Blob {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> flatstream::Result<()> {
        let off = builder.create_vector(&self.0);
        builder.finish(off, None);
        Ok(())
    }
}

// A wrapper policy to log reclamation events to stdout
struct LoggingPolicy<P>(P);

impl<P: MemoryPolicy> MemoryPolicy for LoggingPolicy<P> {
    fn should_reset(
        &mut self,
        last_message_size: usize,
        current_capacity: usize,
    ) -> Option<ReclamationReason> {
        self.0.should_reset(last_message_size, current_capacity)
    }

    fn on_reclaim(&mut self, info: &ReclamationInfo) {
        println!(
            " [!] Memory Reclaimed! Reason: {:?} | Size: {} -> {} bytes",
            info.reason, info.capacity_before, info.capacity_after
        );
        self.0.on_reclaim(info);
    }
}

fn main() -> flatstream::Result<()> {
    // Use a sink that discards data for the example, or a file
    let sink = std::io::sink();
    
    // Configure the policy:
    // - Reset if capacity is >= 4x the current message size
    // - Wait for 10 consecutive small messages before resetting
    let mut base_policy = AdaptiveWatermarkPolicy::default();
    base_policy.shrink_multiple = 4;
    base_policy.messages_to_wait = 10;
    base_policy.cooldown = None; // No time-based cooldown for this deterministic demo

    // Wrap it to add logging
    let policy = LoggingPolicy(base_policy);

    println!("=== Adaptive Memory Policy Example ===");
    println!("1. Initializing writer with default capacity (16KB)...");

    let mut writer = StreamWriter::builder(sink, DefaultFramer)
        .with_policy(policy)
        .with_default_capacity(16 * 1024)
        .build();

    // 1. Write a burst of LARGE messages (1 MB)
    println!("2. Writing large message (1MB) to force buffer growth...");
    let large_blob = Blob(vec![0u8; 1024 * 1024]);
    writer.write(&large_blob)?;

    println!("   (Builder capacity should now be >= 1MB)");

    // 2. Write a stream of SMALL messages (100 bytes)
    println!("3. Writing small messages (100 bytes) to trigger hysteresis...");
    let small_blob = Blob(vec![0u8; 100]);

    for i in 1..=15 {
        writer.write(&small_blob)?;
        
        // The policy is configured to wait for 10 messages.
        // On the 10th small message, we expect a reset.
        if i == 10 {
            println!("   -> Message 10 written (expect reset log above)");
        }
    }

    println!("4. Done.");
    Ok(())
}

