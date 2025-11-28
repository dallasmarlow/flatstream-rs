use flatbuffers::FlatBufferBuilder;
use flatstream::policy::{
    AdaptiveWatermarkPolicy, MemoryPolicy, NoOpPolicy, ReclamationInfo, ReclamationReason,
};
use flatstream::{DefaultFramer, StreamSerialize, StreamWriter};
use std::io::Cursor;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

// A wrapper policy that delegates decision making but captures callback events.
struct ObservingPolicy<P> {
    inner: P,
    callback: Box<dyn Fn(&ReclamationInfo) + Send + Sync>,
}

impl<P: MemoryPolicy> MemoryPolicy for ObservingPolicy<P> {
    fn should_reset(
        &mut self,
        last_message_size: usize,
        current_capacity: usize,
    ) -> Option<ReclamationReason> {
        self.inner.should_reset(last_message_size, current_capacity)
    }

    fn on_reclaim(&mut self, info: &ReclamationInfo) {
        (self.callback)(info);
        self.inner.on_reclaim(info);
    }
}

#[derive(Clone)]
struct TestData(Vec<u8>);

impl StreamSerialize for TestData {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> flatstream::Result<()> {
        let off = builder.create_vector(&self.0);
        builder.finish(off, None);
        Ok(())
    }
}

#[test]
fn test_adaptive_policy_resets_builder() {
    // Setup:
    // 1. Create a writer with an AdaptiveWatermarkPolicy wrapped in an observer.
    // 2. Policy configured to shrink if capacity > 2 * message_size, after 3 messages.

    let reset_count = Arc::new(AtomicUsize::new(0));
    let last_capacity = Arc::new(AtomicUsize::new(0));

    let r_count = reset_count.clone();
    let r_cap = last_capacity.clone();

    let mut base_policy = AdaptiveWatermarkPolicy::default();
    base_policy.size_ratio_threshold = 2;
    base_policy.messages_to_wait = 3;

    let policy = ObservingPolicy {
        inner: base_policy,
        callback: Box::new(move |info| {
            r_count.fetch_add(1, Ordering::Relaxed);
            r_cap.store(info.capacity_before, Ordering::Relaxed);
        }),
    };

    let mut buffer = Vec::new();
    let mut writer = StreamWriter::builder(Cursor::new(&mut buffer), DefaultFramer)
        .with_memory_policy(policy)
        .with_default_capacity(1024)
        .build();

    // 1. Write a LARGE message to force buffer growth.
    // 10KB message. Builder grows to >= 10KB.
    let large_data = TestData(vec![1u8; 10 * 1024]);
    writer.write(&large_data).unwrap();

    // Verify no reset yet
    assert_eq!(reset_count.load(Ordering::Relaxed), 0);

    // 2. Write SMALL messages to trigger hysteresis count.
    // 100B message.
    // Capacity (~10KB) > 100 * 2 (200). Overprovision condition met.
    // messages_to_wait is 3.

    let small_data = TestData(vec![2u8; 100]);

    // Write 1 (count=1)
    writer.write(&small_data).unwrap();
    assert_eq!(reset_count.load(Ordering::Relaxed), 0);

    // Write 2 (count=2)
    writer.write(&small_data).unwrap();
    assert_eq!(reset_count.load(Ordering::Relaxed), 0);

    // Write 3 (count=3) -> RESET TRIGGER!
    writer.write(&small_data).unwrap();

    // 3. Assert reset occurred
    assert_eq!(reset_count.load(Ordering::Relaxed), 1);

    // The capacity before reset should have been large (>= 10KB)
    let cap_before = last_capacity.load(Ordering::Relaxed);
    assert!(cap_before >= 10 * 1024);

    // 4. Verify data integrity
    // Read back all messages and verify contents
    let mut reader =
        flatstream::StreamReader::new(Cursor::new(buffer), flatstream::DefaultDeframer);
    let mut messages = reader.messages();

    // Msg 1: Large
    let m1 = messages.next().unwrap().unwrap();
    // Verify content (skip flatbuffer overhead validation for now, just size check approx)
    // FlatBuffers adds minimal overhead (vector size + header).
    assert!(m1.len() >= 10 * 1024);

    // Msg 2, 3, 4: Small
    for _ in 0..3 {
        let m = messages.next().unwrap().unwrap();
        assert!(m.len() >= 100);
    }
}

#[test]
fn test_noop_policy_never_resets() {
    let reset_count = Arc::new(AtomicUsize::new(0));
    let r_count = reset_count.clone();

    let policy = ObservingPolicy {
        inner: NoOpPolicy,
        callback: Box::new(move |_| {
            r_count.fetch_add(1, Ordering::Relaxed);
        }),
    };

    let mut buffer = Vec::new();
    let mut writer = StreamWriter::builder(Cursor::new(&mut buffer), DefaultFramer)
        .with_memory_policy(policy)
        .build();

    // Write mixed sizes
    writer.write(&TestData(vec![0u8; 1024])).unwrap();
    for _ in 0..10 {
        writer.write(&TestData(vec![0u8; 10])).unwrap();
    }

    assert_eq!(reset_count.load(Ordering::Relaxed), 0);
}
