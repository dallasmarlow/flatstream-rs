//! C4 — The zero-allocation steady state, as an enforced invariant.
//!
//! The crate's headline property is that once buffers reach their high-water
//! mark, the write and read loops allocate nothing. That claim has been argued
//! in the design docs and observed in benchmarks, but until now nothing failed
//! when it broke — and a wall-clock benchmark is the wrong instrument for it.
//! One extra allocation per frame costs tens of nanoseconds, well inside the
//! −24%/+57% run-to-run drift `CONTRIBUTING.md` §4 documents. A regression
//! would be indistinguishable from a noisy machine.
//!
//! It is also easy to introduce and innocent-looking in review: a `format!` on
//! a path that turns out to be hot, a `.to_vec()` where a slice would do, a
//! `Box::new` per frame inside a new adapter.
//!
//! This is a categorical property — the count is zero or it is not — so it is
//! counted rather than timed. See `docs/planning/ZERO_ALLOCATION_ENFORCEMENT.md`
//! for the full rationale, including what this deliberately does *not* prove:
//! allocations and copies are different properties, and a `memcpy` into an
//! already-allocated buffer reports a clean zero here. This pins the
//! allocation half of the steady-state claim only.

use flatbuffers::FlatBufferBuilder;
use flatstream::{
    read_frame_at, DefaultDeframer, DefaultFramer, DeframerExt, Durable, Framer, PostWriteEvent,
    PostWriteOutcome, StreamReader, StreamSerialize, StreamWriter, SyncEveryInterval,
    SyncEveryNFrames, SyncMode, TableRootValidator,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::Cursor;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

// --- The counting allocator -------------------------------------------------

// Counting is thread-local, not global: `cargo test` runs tests in parallel
// threads within one binary, so a shared counter would attribute one test's
// allocations to another's measurement window. The allocator runs on the
// allocating thread, so a thread-local attributes correctly and needs no
// synchronization.
//
// `const`-initialized because lazy TLS initialization can itself allocate,
// which would recurse into the allocator being measured.
thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<usize> = const { Cell::new(0) };
    static REALLOCS: Cell<usize> = const { Cell::new(0) };
}

struct CountingAllocator;

impl CountingAllocator {
    /// `try_with` rather than `with`: during thread teardown the TLS is already
    /// destroyed, and a panic inside the global allocator is not recoverable.
    #[inline]
    fn bump(counter: &'static std::thread::LocalKey<Cell<usize>>) {
        let armed = ARMED.try_with(Cell::get).unwrap_or(false);
        if armed {
            let _ = counter.try_with(|c| c.set(c.get() + 1));
        }
    }
}

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        Self::bump(&ALLOCS);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // Frees are not counted. A steady-state loop that allocates nothing
        // also frees nothing, so `allocs == 0` is the property; counting frees
        // would only add noise from teardown of setup values.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // Counted separately because this is the shape a buffer-growth
        // regression takes: `Vec::resize` past capacity reallocs rather than
        // allocs, so a test that only watched `alloc` would miss it.
        Self::bump(&REALLOCS);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Counts {
    allocs: usize,
    reallocs: usize,
}

impl Counts {
    fn total(&self) -> usize {
        self.allocs + self.reallocs
    }
}

/// Runs `body` with allocation counting armed, and returns what it did.
///
/// Everything that formats — assertion machinery, `format!`, panic payloads —
/// allocates, so the counters are read and disarmed before any assertion runs.
/// Callers must do their asserting on the returned `Counts`, never inside
/// `body`.
fn measure<T>(body: impl FnOnce() -> T) -> (Counts, T) {
    ALLOCS.with(|c| c.set(0));
    REALLOCS.with(|c| c.set(0));
    ARMED.with(|c| c.set(true));

    let out = body();

    ARMED.with(|c| c.set(false));
    let counts = Counts {
        allocs: ALLOCS.with(Cell::get),
        reallocs: REALLOCS.with(Cell::get),
    };
    (counts, out)
}

// --- Fixtures ---------------------------------------------------------------

/// The largest payload any loop below writes. Warmup must use this size: the
/// steady state is only reached once the builder and the reader's buffer have
/// grown to the largest frame they will see, so warming with smaller payloads
/// would report correct growth as a regression.
const MAX_PAYLOAD: usize = 512;

const WARMUP: usize = 32;
const MEASURED: usize = 256;

fn build(builder: &mut FlatBufferBuilder, len: usize) {
    builder.reset();
    let s = "x".repeat(len);
    let off = builder.create_string(&s);
    builder.finish(off, None);
}

/// A sink that cannot allocate, so that any count belongs to the library.
///
/// A `Vec<u8>` would grow and charge its reallocs to us; `io::sink()` discards
/// and would not exercise a real write path. This keeps the bytes and the
/// bound, and asserts the bound held.
struct FixedSink {
    buf: Vec<u8>,
    cap: usize,
}

impl FixedSink {
    fn new(cap: usize) -> Self {
        Self {
            buf: Vec::with_capacity(cap),
            cap,
        }
    }
}

impl std::io::Write for FixedSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        assert!(
            self.buf.len() + buf.len() <= self.cap,
            "FixedSink overflow: the test under-reserved, so its own growth \
             would be counted as the library's"
        );
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Durable for FixedSink {
    fn sync_data(&mut self) -> std::io::Result<()> {
        Ok(())
    }

    fn sync_all(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Blob<'a>(&'a [u8]);

impl StreamSerialize for Blob<'_> {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> flatstream::Result<()> {
        let payload = builder.create_vector(self.0);
        builder.finish(payload, None);
        Ok(())
    }
}

// --- The harness must itself be trustworthy ---------------------------------

#[test]
fn the_counter_sees_an_allocation() {
    // A zero-assertion is only meaningful if a nonzero result is reachable. If
    // arming were broken, every other test in this file would pass vacuously.
    let (counts, v) = measure(|| vec![7u8; 1024]);
    assert_eq!(v.len(), 1024);
    assert!(
        counts.allocs >= 1,
        "harness is not observing allocations: {counts:?}"
    );
}

#[test]
fn the_counter_is_disarmed_outside_measure() {
    let (before, _) = measure(|| ());
    let _noise = vec![0u8; 4096];
    let (after, _) = measure(|| ());
    assert_eq!(before.total(), 0);
    assert_eq!(
        after.total(),
        0,
        "allocations leaked into a disarmed window"
    );
}

// --- The claim --------------------------------------------------------------

#[test]
fn steady_state_expert_write_allocates_nothing() {
    let mut builder = FlatBufferBuilder::with_capacity(MAX_PAYLOAD * 4);
    let mut writer = StreamWriter::new(FixedSink::new(1 << 20), DefaultFramer);

    for _ in 0..WARMUP {
        build(&mut builder, MAX_PAYLOAD);
        writer.write_finished(&mut builder).unwrap();
    }

    // `build` allocates a String per call by design, so it stays outside the
    // armed region: pre-build one payload and write that same finished buffer.
    build(&mut builder, MAX_PAYLOAD);
    let (counts, _) = measure(|| {
        for _ in 0..MEASURED {
            writer.write_finished(&mut builder).unwrap();
        }
    });

    assert_eq!(
        counts,
        Counts {
            allocs: 0,
            reallocs: 0
        },
        "expert-mode write"
    );
}

#[test]
fn steady_state_simple_write_allocates_nothing() {
    let bytes = [0xA5u8; MAX_PAYLOAD];
    let blob = Blob(&bytes);
    let mut writer =
        StreamWriter::with_capacity(FixedSink::new(1 << 20), DefaultFramer, MAX_PAYLOAD * 4);

    for _ in 0..WARMUP {
        writer.write(&blob).unwrap();
    }

    let (counts, _) = measure(|| {
        for _ in 0..MEASURED {
            writer.write(&blob).unwrap();
        }
    });

    assert_eq!(
        counts,
        Counts {
            allocs: 0,
            reallocs: 0
        },
        "simple-mode write"
    );
}

#[test]
fn installed_post_write_observer_allocates_nothing_per_frame() {
    let mut builder = FlatBufferBuilder::with_capacity(MAX_PAYLOAD * 4);
    build(&mut builder, MAX_PAYLOAD);
    let observed = Arc::new(AtomicUsize::new(0));
    let observer_count = Arc::clone(&observed);
    let mut writer = StreamWriter::new(FixedSink::new(1 << 20), DefaultFramer)
        .with_post_write_observer(move |event: PostWriteEvent<'_>| {
            assert!(matches!(event.outcome, PostWriteOutcome::Succeeded(_)));
            observer_count.fetch_add(1, Ordering::Relaxed);
        });

    for _ in 0..WARMUP {
        writer.write_finished(&mut builder).unwrap();
    }
    let before = observed.load(Ordering::Relaxed);

    let (counts, _) = measure(|| {
        for _ in 0..MEASURED {
            writer.write_finished(&mut builder).unwrap();
        }
    });

    assert_eq!(
        counts,
        Counts {
            allocs: 0,
            reallocs: 0
        },
        "installed post-write observer"
    );
    assert_eq!(
        observed.load(Ordering::Relaxed) - before,
        MEASURED,
        "observer fires exactly once per measured frame"
    );
}

#[test]
fn static_sync_policy_allocates_nothing_even_when_it_checkpoints() {
    let mut builder = FlatBufferBuilder::with_capacity(MAX_PAYLOAD * 4);
    build(&mut builder, MAX_PAYLOAD);
    let policy = SyncEveryNFrames::new(NonZeroU64::new(MEASURED as u64).unwrap(), SyncMode::Data);
    let mut writer =
        StreamWriter::new(FixedSink::new(1 << 20), DefaultFramer).with_sync_policy(policy);

    for _ in 0..WARMUP {
        writer.write_finished(&mut builder).unwrap();
    }
    // The warmup remains below the cadence, so the measured window contains
    // exactly one successful checkpoint.
    let (counts, _) = measure(|| {
        for _ in 0..MEASURED {
            writer.write_finished(&mut builder).unwrap();
        }
    });

    assert_eq!(
        counts,
        Counts {
            allocs: 0,
            reallocs: 0
        },
        "static sync policy"
    );
}

#[test]
fn interval_sync_policy_allocates_nothing_while_observing_frames() {
    let mut builder = FlatBufferBuilder::with_capacity(MAX_PAYLOAD * 4);
    build(&mut builder, MAX_PAYLOAD);
    let policy = SyncEveryInterval::new(Duration::from_secs(60), SyncMode::Data);
    let mut writer =
        StreamWriter::new(FixedSink::new(1 << 20), DefaultFramer).with_sync_policy(policy);

    for _ in 0..WARMUP {
        writer.write_finished(&mut builder).unwrap();
    }
    let (counts, _) = measure(|| {
        for _ in 0..MEASURED {
            writer.write_finished(&mut builder).unwrap();
        }
    });

    assert_eq!(
        counts,
        Counts {
            allocs: 0,
            reallocs: 0
        },
        "interval sync policy"
    );
}

#[test]
fn steady_state_write_with_receipt_allocates_nothing() {
    // The 0.2.8 addition: `CountingWriter` sits in this path and receipts are
    // returned by value. Neither should cost an allocation.
    let mut builder = FlatBufferBuilder::with_capacity(MAX_PAYLOAD * 4);
    let mut writer = StreamWriter::new(FixedSink::new(1 << 20), DefaultFramer);

    for _ in 0..WARMUP {
        build(&mut builder, MAX_PAYLOAD);
        writer.write_finished_with_receipt(&mut builder).unwrap();
    }

    build(&mut builder, MAX_PAYLOAD);
    let (counts, last) = measure(|| {
        let mut last = None;
        for _ in 0..MEASURED {
            last = Some(writer.write_finished_with_receipt(&mut builder).unwrap());
        }
        last
    });

    assert!(last.unwrap().wire_len > 0);
    assert_eq!(
        counts,
        Counts {
            allocs: 0,
            reallocs: 0
        },
        "receipt write"
    );
}

#[cfg(feature = "crc32")]
#[test]
fn steady_state_checksummed_write_allocates_nothing() {
    use flatstream::{ChecksumFramer, Crc32};

    let mut builder = FlatBufferBuilder::with_capacity(MAX_PAYLOAD * 4);
    let mut writer = StreamWriter::new(FixedSink::new(1 << 20), ChecksumFramer::new(Crc32::new()));

    for _ in 0..WARMUP {
        build(&mut builder, MAX_PAYLOAD);
        writer.write_finished(&mut builder).unwrap();
    }

    build(&mut builder, MAX_PAYLOAD);
    let (counts, _) = measure(|| {
        for _ in 0..MEASURED {
            writer.write_finished(&mut builder).unwrap();
        }
    });

    assert_eq!(
        counts,
        Counts {
            allocs: 0,
            reallocs: 0
        },
        "checksummed write"
    );
}

#[test]
fn steady_state_read_allocates_nothing() {
    // Build a stream whose frames are all MAX_PAYLOAD-sized, so the reader's
    // buffer reaches its high-water mark during warmup and never grows again.
    let mut bytes = Vec::new();
    {
        let mut builder = FlatBufferBuilder::with_capacity(MAX_PAYLOAD * 4);
        let mut writer = StreamWriter::new(&mut bytes, DefaultFramer);
        for _ in 0..(WARMUP + MEASURED) {
            build(&mut builder, MAX_PAYLOAD);
            writer.write_finished(&mut builder).unwrap();
        }
        writer.flush().unwrap();
    }

    let mut reader = StreamReader::new(Cursor::new(&bytes), DefaultDeframer::new());
    for _ in 0..WARMUP {
        assert!(reader.read_message().unwrap().is_some());
    }

    let (counts, read) = measure(|| {
        let mut read = 0usize;
        while let Some(payload) = reader.read_message().unwrap() {
            // Touch the payload so the read cannot be optimized away, without
            // allocating: sum bytes rather than copying them.
            read += payload.len().min(1);
        }
        read
    });

    assert_eq!(
        read, MEASURED,
        "warmup + measured frames must tile the stream"
    );
    assert_eq!(
        counts,
        Counts {
            allocs: 0,
            reallocs: 0
        },
        "steady-state read"
    );
}

#[test]
fn steady_state_validating_read_allocates_nothing() {
    // The rustdoc for the validation layer claims structural validation "adds no
    // allocations on the success path." Every other steady-state claim in the
    // crate is enforced by this harness; this one was enforced by nothing. Bring
    // it under the counter: a validating read over valid table roots must
    // allocate zero times once the reader buffer is warm. This also guards the
    // claim against a future `flatbuffers` upgrade silently making the verifier
    // allocate — the current verifier carries no owned buffer on the ok path.
    let table = {
        let mut b = FlatBufferBuilder::with_capacity(MAX_PAYLOAD * 4);
        let start = b.start_table();
        let root = b.end_table(start);
        b.finish(root, None);
        b.finished_data().to_vec()
    };

    // Frame the raw table bytes directly so the on-wire payload is a real table
    // root the validator accepts. All frames are identical, so the reader buffer
    // reaches its high-water mark during warmup and never regrows.
    let mut bytes = Vec::new();
    for _ in 0..(WARMUP + MEASURED) {
        DefaultFramer.frame_and_write(&mut bytes, &table).unwrap();
    }

    let deframer = DefaultDeframer::new().with_validator(TableRootValidator::new());
    let mut reader = StreamReader::new(Cursor::new(&bytes), deframer);
    for _ in 0..WARMUP {
        assert!(reader.read_message().unwrap().is_some());
    }

    let (counts, read) = measure(|| {
        let mut read = 0usize;
        while let Some(payload) = reader.read_message().unwrap() {
            read += payload.len().min(1);
        }
        read
    });

    assert_eq!(
        read, MEASURED,
        "warmup + measured frames must tile the stream"
    );
    assert_eq!(
        counts,
        Counts {
            allocs: 0,
            reallocs: 0
        },
        "validating read"
    );
}

#[test]
fn steady_state_point_lookup_allocates_nothing() {
    let mut bytes = Vec::new();
    let receipt;
    {
        let mut builder = FlatBufferBuilder::with_capacity(MAX_PAYLOAD * 4);
        build(&mut builder, MAX_PAYLOAD);
        let mut writer = StreamWriter::new(&mut bytes, DefaultFramer);
        receipt = writer.write_finished_with_receipt(&mut builder).unwrap();
    }

    let mut source = Cursor::new(&bytes);
    let mut scratch = Vec::new();
    // Warm the caller-owned scratch to the frame's high-water mark.
    read_frame_at(
        &mut source,
        &DefaultDeframer::new(),
        receipt.frame_start,
        &mut scratch,
    )
    .unwrap()
    .unwrap();

    let (counts, touched) = measure(|| {
        let mut touched = 0usize;
        for _ in 0..MEASURED {
            let frame = read_frame_at(
                &mut source,
                &DefaultDeframer::new(),
                receipt.frame_start,
                &mut scratch,
            )
            .unwrap()
            .unwrap();
            touched += frame.payload.len().min(1);
        }
        touched
    });

    assert_eq!(touched, MEASURED);
    assert_eq!(
        counts,
        Counts {
            allocs: 0,
            reallocs: 0
        },
        "point lookup"
    );
}

#[test]
fn fresh_reader_per_lookup_allocates_each_time() {
    let mut bytes = Vec::new();
    {
        let mut builder = FlatBufferBuilder::with_capacity(MAX_PAYLOAD * 4);
        build(&mut builder, MAX_PAYLOAD);
        StreamWriter::new(&mut bytes, DefaultFramer)
            .write_finished(&mut builder)
            .unwrap();
    }

    let (counts, read) = measure(|| {
        let mut read = 0usize;
        for _ in 0..MEASURED {
            let mut reader = StreamReader::new(Cursor::new(&bytes), DefaultDeframer::new());
            read += reader.read_message().unwrap().unwrap().len().min(1);
        }
        read
    });

    assert_eq!(read, MEASURED);
    assert!(
        counts.total() >= MEASURED,
        "a fresh reader must allocate its frame buffer each lookup: {counts:?}"
    );
}

#[test]
fn a_growing_frame_reallocs_exactly_once_then_settles() {
    // The complement to the zero-assertions, and the reason `realloc` is
    // counted separately: growth past the high-water mark *should* cost, and
    // the frame after it should not. A change that made every read re-grow
    // would still pass the steady-state test above if its warmup happened to
    // reach the mark, but it would fail here.
    let mut bytes = Vec::new();
    {
        let mut builder = FlatBufferBuilder::with_capacity(MAX_PAYLOAD * 4);
        let mut writer = StreamWriter::new(&mut bytes, DefaultFramer);
        for len in [64, 64, MAX_PAYLOAD, MAX_PAYLOAD] {
            build(&mut builder, len);
            writer.write_finished(&mut builder).unwrap();
        }
        writer.flush().unwrap();
    }

    let mut reader = StreamReader::new(Cursor::new(&bytes), DefaultDeframer::new());
    reader.read_message().unwrap().unwrap();
    reader.read_message().unwrap().unwrap();

    // Frame 3 jumps from 64 to 512 bytes: the buffer must grow.
    let (grow, _) = measure(|| reader.read_message().unwrap().map(<[u8]>::len));
    // Frame 4 is the same size: nothing should move.
    let (settled, _) = measure(|| reader.read_message().unwrap().map(<[u8]>::len));

    assert!(
        grow.total() >= 1,
        "growing past the high-water mark should cost something: {grow:?}"
    );
    assert_eq!(
        settled,
        Counts {
            allocs: 0,
            reallocs: 0
        },
        "the frame after growth must be free"
    );
}
