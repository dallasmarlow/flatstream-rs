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

    fn baseline_capacity(&self) -> usize {
        self.inner.baseline_capacity()
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

    let policy = ObservingPolicy {
        inner: AdaptiveWatermarkPolicy::new(2, 3).with_baseline(1024),
        callback: Box::new(move |info| {
            r_count.fetch_add(1, Ordering::Relaxed);
            r_cap.store(info.capacity_before, Ordering::Relaxed);
        }),
    };

    let mut buffer = Vec::new();
    let mut writer =
        StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer).with_memory_policy(policy);

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

    // The capacity before reset should have been large (>= 10KB).
    // NOTE: this assertion doubles as the regression guard for the writer's
    // capacity probe (`mut_finished_buffer().len()` used as effective builder
    // capacity — an implementation detail of the `flatbuffers` crate, 25.9.23
    // at time of writing). If upstream changes that semantic, this fails loudly.
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

/// Serializes an item standalone to obtain the exact payload bytes the stream
/// should contain — lets the reader-side tests assert byte-identical payloads
/// across a reclaim boundary.
fn payload_of(item: &TestData) -> Vec<u8> {
    let mut b = FlatBufferBuilder::new();
    item.serialize(&mut b).unwrap();
    b.finished_data().to_vec()
}

#[test]
fn test_reader_policy_reclaims_buffer_without_corrupting_stream() {
    // The reader defers its shrink to the start of the read *after* the policy
    // fires, so the payload returned at trigger time must remain valid. This
    // test verifies both the reclamation and that every payload — including the
    // ones straddling the shrink boundary — is byte-identical to what was written.

    let items: Vec<TestData> = std::iter::once(TestData(vec![1u8; 10 * 1024]))
        .chain((0..6).map(|i| TestData(vec![10 + i as u8; 100])))
        .collect();
    let expected: Vec<Vec<u8>> = items.iter().map(payload_of).collect();

    let mut buffer = Vec::new();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
        for item in &items {
            writer.write(item).unwrap();
        }
    }

    let reset_count = Arc::new(AtomicUsize::new(0));
    let last_info = Arc::new(AtomicUsize::new(0));
    let r_count = reset_count.clone();
    let r_before = last_info.clone();

    let policy = ObservingPolicy {
        inner: AdaptiveWatermarkPolicy::new(2, 3).with_baseline(1024),
        callback: Box::new(move |info| {
            r_count.fetch_add(1, Ordering::Relaxed);
            r_before.store(info.capacity_before, Ordering::Relaxed);
        }),
    };

    let mut reader =
        flatstream::StreamReader::new(Cursor::new(&buffer), flatstream::DefaultDeframer)
            .with_memory_policy(policy);

    let mut seen = Vec::new();
    let mut messages = reader.messages();
    while let Some(payload) = messages.next().unwrap() {
        seen.push(payload.to_vec());
    }

    // Every payload round-trips byte-identically, across the shrink boundary.
    assert_eq!(seen, expected);

    // The large frame grew the buffer past 10 KiB; three consecutive small
    // reads (capacity >= 2x payload) fired the policy exactly once. After the
    // deferred shrink, capacity sits at the 1 KiB baseline, so the gate stops
    // consulting the policy — no further resets.
    assert_eq!(reset_count.load(Ordering::Relaxed), 1);
    assert!(last_info.load(Ordering::Relaxed) >= 10 * 1024);

    // The buffer really was reclaimed: well below the 10 KiB high-water mark.
    assert!(reader.buffer_capacity() < 10 * 1024);
    assert!(reader.buffer_capacity() >= 100);
}

#[test]
fn test_writer_policy_with_custom_builder_factory() {
    // The factory variant exists for custom allocators; the contract under test
    // is that a reclaim rebuilds the builder through the caller's factory
    // (exactly once here) and that the stream stays intact.

    let factory_calls = Arc::new(AtomicUsize::new(0));
    let f_calls = factory_calls.clone();

    let mut buffer = Vec::new();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer)
            .with_memory_policy_and_factory(
                AdaptiveWatermarkPolicy::new(2, 3).with_baseline(1024),
                move |cap| {
                    f_calls.fetch_add(1, Ordering::Relaxed);
                    FlatBufferBuilder::with_capacity(cap)
                },
            );

        writer.write(&TestData(vec![1u8; 10 * 1024])).unwrap();
        for _ in 0..3 {
            writer.write(&TestData(vec![2u8; 100])).unwrap();
        }
    }

    // The third small message fired the policy; the builder was rebuilt through
    // the factory exactly once (the gate prevents re-firing at the baseline).
    assert_eq!(factory_calls.load(Ordering::Relaxed), 1);

    // All four messages survive intact.
    let mut reader =
        flatstream::StreamReader::new(Cursor::new(&buffer), flatstream::DefaultDeframer);
    let mut count = 0;
    reader
        .process_all(|payload| {
            assert!(!payload.is_empty());
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 4);
}

#[test]
fn test_reader_with_policy_is_send() {
    fn assert_send<T: Send>(_: &T) {}
    let reader =
        flatstream::StreamReader::new(Cursor::new(Vec::new()), flatstream::DefaultDeframer)
            .with_memory_policy(AdaptiveWatermarkPolicy::default());
    assert_send(&reader);
}

#[test]
fn test_write_finished_ignores_installed_policy() {
    // Expert mode owns its builder: an installed policy must never be consulted
    // (let alone fire) for write_finished(), no matter how eager the policy is.
    struct EagerCountingPolicy {
        consults: Arc<AtomicUsize>,
        reclaims: Arc<AtomicUsize>,
    }
    impl MemoryPolicy for EagerCountingPolicy {
        fn should_reset(&mut self, _: usize, _: usize) -> Option<ReclamationReason> {
            self.consults.fetch_add(1, Ordering::Relaxed);
            Some(ReclamationReason::MessageCount) // would fire on every consult
        }
        fn on_reclaim(&mut self, _: &ReclamationInfo) {
            self.reclaims.fetch_add(1, Ordering::Relaxed);
        }

        fn baseline_capacity(&self) -> usize {
            1 // keep the gate open for any capacity — still must not be consulted
        }
    }

    let consults = Arc::new(AtomicUsize::new(0));
    let reclaims = Arc::new(AtomicUsize::new(0));
    let policy = EagerCountingPolicy {
        consults: consults.clone(),
        reclaims: reclaims.clone(),
    };

    let mut buffer = Vec::new();
    {
        let mut writer =
            StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer).with_memory_policy(policy);

        let mut b = FlatBufferBuilder::new();
        for i in 0..5 {
            b.reset();
            TestData(vec![i as u8; 10 * 1024])
                .serialize(&mut b)
                .unwrap();
            writer.write_finished(&mut b).unwrap();
        }
    }

    assert_eq!(
        consults.load(Ordering::Relaxed),
        0,
        "write_finished must never consult the policy"
    );
    assert_eq!(reclaims.load(Ordering::Relaxed), 0);

    // The stream is intact.
    let mut reader =
        flatstream::StreamReader::new(Cursor::new(&buffer), flatstream::DefaultDeframer);
    let mut count = 0;
    reader
        .process_all(|p| {
            assert!(p.len() >= 10 * 1024);
            count += 1;
            Ok(())
        })
        .unwrap();
    assert_eq!(count, 5);
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
    let mut writer =
        StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer).with_memory_policy(policy);

    // Write mixed sizes
    writer.write(&TestData(vec![0u8; 1024])).unwrap();
    for _ in 0..10 {
        writer.write(&TestData(vec![0u8; 10])).unwrap();
    }

    assert_eq!(reset_count.load(Ordering::Relaxed), 0);
}
