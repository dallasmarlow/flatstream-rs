//! Example demonstrating the Adaptive Memory Policy for the StreamWriter.
//!
//! This example simulates a "bursty" workload where a large message causes
//! buffer expansion, followed by many small messages that eventually trigger
//! a memory reclamation event (resetting the internal builder).

use flatbuffers::FlatBufferBuilder;
use flatstream::policy::{
    AdaptiveWatermarkPolicy, MemoryPolicy, ReclamationInfo, ReclamationReason,
};
use flatstream::{DefaultFramer, StreamSerialize, StreamWriter};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const BASELINE: usize = 16 * 1024;
const LARGE: usize = 1024 * 1024;
const HYSTERESIS: usize = 10;

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

/// What the example asserts about a reclamation: which small message triggered
/// it, and how far the builder shrank.
#[derive(Debug)]
struct Reclaim {
    at_small_message: usize,
    capacity_before: usize,
    capacity_after: usize,
}

/// Wraps a policy to log reclamation events *and* record them, so the example
/// can assert the hysteresis actually behaved as advertised rather than just
/// printing and hoping.
///
/// `MemoryPolicy: Send`, so the shared handles are `Arc`-based even though this
/// example is single-threaded.
struct RecordingPolicy<P> {
    inner: P,
    /// Set by the write loop before each small message, read on reclaim.
    small_message_ordinal: Arc<AtomicUsize>,
    events: Arc<Mutex<Vec<Reclaim>>>,
}

impl<P: MemoryPolicy> MemoryPolicy for RecordingPolicy<P> {
    fn should_reset(
        &mut self,
        last_message_size: usize,
        current_capacity: usize,
    ) -> Option<ReclamationReason> {
        self.inner.should_reset(last_message_size, current_capacity)
    }

    fn on_reclaim(&mut self, info: &ReclamationInfo) {
        println!(
            " [!] Memory Reclaimed! Reason: {:?} | Size: {} -> {} bytes",
            info.reason, info.capacity_before, info.capacity_after
        );
        self.events.lock().expect("events lock").push(Reclaim {
            at_small_message: self.small_message_ordinal.load(Ordering::Relaxed),
            capacity_before: info.capacity_before,
            capacity_after: info.capacity_after,
        });
        self.inner.on_reclaim(info);
    }

    fn baseline_capacity(&self) -> usize {
        self.inner.baseline_capacity()
    }
}

fn main() -> flatstream::Result<()> {
    // Use a sink that discards data for the example, or a file
    let sink = std::io::sink();

    // Configure the policy:
    // - Reset if capacity is >= 4x the current message size
    // - Wait for 10 consecutive small messages before resetting
    // (no time-based cooldown, keeping this demo deterministic)
    let base_policy = AdaptiveWatermarkPolicy::new(4, HYSTERESIS as u32).with_baseline(BASELINE);

    // Wrap it so reclamation events are recorded, not merely printed.
    let ordinal = Arc::new(AtomicUsize::new(0));
    let events = Arc::new(Mutex::new(Vec::new()));
    let policy = RecordingPolicy {
        inner: base_policy,
        small_message_ordinal: Arc::clone(&ordinal),
        events: Arc::clone(&events),
    };

    println!("=== Adaptive Memory Policy Example ===");
    println!("1. Initializing writer with default capacity (16KB)...");

    let mut writer = StreamWriter::new(sink, DefaultFramer).with_memory_policy(policy);

    // 1. Write a burst of LARGE messages (1 MB)
    println!("2. Writing large message (1MB) to force buffer growth...");
    let large_blob = Blob(vec![0u8; LARGE]);
    writer.write(&large_blob)?;
    assert!(
        events.lock().expect("events lock").is_empty(),
        "the large message must not trigger a reclaim: the builder is correctly sized for it"
    );

    // 2. Write a stream of SMALL messages (100 bytes)
    println!("3. Writing small messages (100 bytes) to trigger hysteresis...");
    let small_blob = Blob(vec![0u8; 100]);

    for i in 1..=15 {
        ordinal.store(i, Ordering::Relaxed);
        writer.write(&small_blob)?;

        // The policy waits for `HYSTERESIS` consecutive small messages, so
        // nothing may fire before then.
        if i < HYSTERESIS {
            assert!(
                events.lock().expect("events lock").is_empty(),
                "reclaimed after only {i} small messages; hysteresis of {HYSTERESIS} was not honored"
            );
        }
    }

    // 3. Assert the behavior the example claims to demonstrate.
    let events = events.lock().expect("events lock");
    assert_eq!(
        events.len(),
        1,
        "expected exactly one reclaim; the builder sits at baseline afterwards, \
         so the policy should not be consulted again. Got: {events:?}"
    );
    let ev = &events[0];
    assert_eq!(
        ev.at_small_message, HYSTERESIS,
        "reclaim should fire on small message {HYSTERESIS}, not {}",
        ev.at_small_message
    );
    assert!(
        ev.capacity_before >= LARGE,
        "builder should have grown to hold the 1MB message, was {} bytes",
        ev.capacity_before
    );
    assert_eq!(
        ev.capacity_after, BASELINE,
        "reclaim must rebuild at the policy's baseline"
    );

    println!(
        "4. Done — one reclaim on small message {}, {} -> {} bytes ({:.0}x shrink).",
        ev.at_small_message,
        ev.capacity_before,
        ev.capacity_after,
        ev.capacity_before as f64 / ev.capacity_after as f64
    );
    Ok(())
}
