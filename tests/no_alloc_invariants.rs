#![cfg_attr(miri, ignore)]
// Zero-allocation invariant tests using a counting global allocator.
//
// Harness note (fixed 2026-07-04): the previous version gated counting behind a
// `OnceLock<bool>` that `alloc()` initialized to `false` during test *setup*
// allocations, so the later `set(true)` always failed and every assertion
// passed vacuously. Counting is now thread-local, so only the measuring thread
// counts and parallel tests cannot pollute each other. Tests warm buffers to
// their high-water mark first, then measure a steady-state pass — which is the
// invariant the library actually claims (buffers grow once, then are reused).
use flatstream::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::Cursor;

struct CountingAlloc;

// Thread-local counting: only the thread inside `count_allocs` counts, so
// parallel tests cannot pollute each other's measurements. Const-init `Cell`s
// of Copy types compile to plain TLS slots — no lazy init, no destructor
// registration, no allocation from within the allocator itself.
thread_local! {
    static TL_ENABLED: Cell<bool> = const { Cell::new(false) };
    static TL_COUNT: Cell<usize> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        TL_ENABLED.with(|e| {
            if e.get() {
                TL_COUNT.with(|c| c.set(c.get() + 1));
            }
        });
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GA: CountingAlloc = CountingAlloc;

/// Runs `f` with allocation counting enabled on the current thread and returns
/// the number of heap allocations it performed.
fn count_allocs(f: impl FnOnce()) -> usize {
    TL_COUNT.with(|c| c.set(0));
    TL_ENABLED.with(|e| e.set(true));
    f();
    TL_ENABLED.with(|e| e.set(false));
    TL_COUNT.with(|c| c.get())
}

#[test]
fn zero_alloc_in_process_all_and_messages() {
    // Purpose: Verify no heap allocations occur during steady-state read paths
    // (`process_all` and `messages()`).
    let mut out = Vec::new();
    for _ in 0..5 {
        DefaultFramer.frame_and_write(&mut out, b"x").unwrap();
    }

    // process_all path: warm to high-water, rewind, measure a full pass.
    let mut r = StreamReader::new(Cursor::new(&out), DefaultDeframer);
    r.process_all(|_| Ok(())).unwrap();
    r.get_mut().set_position(0);
    let n = count_allocs(|| r.process_all(|_| Ok(())).unwrap());
    assert_eq!(n, 0, "steady-state process_all should not allocate");

    // messages() path
    let mut r = StreamReader::new(Cursor::new(&out), DefaultDeframer);
    r.process_all(|_| Ok(())).unwrap();
    r.get_mut().set_position(0);
    let n = count_allocs(|| {
        let mut it = r.messages();
        while let Some(_p) = it.next().unwrap() {}
    });
    assert_eq!(n, 0, "steady-state messages() should not allocate");
}

#[test]
fn zero_alloc_in_typed_paths() {
    // Purpose: Verify typed read paths (`process_typed`, `typed_messages`) are
    // also zero-allocation in steady state.
    let mut out = Vec::new();
    {
        let mut b = flatbuffers::FlatBufferBuilder::new();
        for _ in 0..5 {
            b.reset();
            let s = b.create_string("x");
            b.finish(s, None);
            DefaultFramer
                .frame_and_write(&mut out, b.finished_data())
                .unwrap();
        }
    }

    struct StrRoot;
    impl<'a> StreamDeserialize<'a> for StrRoot {
        type Root = &'a str;
        fn from_payload(payload: &'a [u8]) -> Result<Self::Root> {
            flatbuffers::root::<&'a str>(payload).map_err(Error::FlatbuffersError)
        }
    }

    // process_typed
    let mut r = StreamReader::new(Cursor::new(&out), DefaultDeframer);
    r.process_typed::<StrRoot, _>(|_| Ok(())).unwrap();
    r.get_mut().set_position(0);
    let n = count_allocs(|| r.process_typed::<StrRoot, _>(|_| Ok(())).unwrap());
    assert_eq!(n, 0, "steady-state process_typed should not allocate");

    // typed_messages iterator
    let mut r = StreamReader::new(Cursor::new(&out), DefaultDeframer);
    r.process_typed::<StrRoot, _>(|_| Ok(())).unwrap();
    r.get_mut().set_position(0);
    let n = count_allocs(|| {
        let mut it = r.typed_messages::<StrRoot>();
        while let Some(_root) = it.next().unwrap() {}
    });
    assert_eq!(n, 0, "steady-state typed_messages should not allocate");
}

#[test]
fn zero_alloc_steady_state_with_policy_installed() {
    // Purpose: An installed-but-not-firing memory policy must preserve the
    // zero-allocation invariant. The consult is pure arithmetic (count-based
    // AdaptiveWatermarkPolicy without a cooldown performs no clock reads), and
    // reclaim_capacity(1) keeps the baseline gate open so the policy really is
    // consulted on every message — the strongest version of the claim.

    // Writer: warm the builder to steady state, then measure.
    let mut w = StreamWriter::new(std::io::sink(), DefaultFramer)
        .with_memory_policy(policy::AdaptiveWatermarkPolicy::new(1_000_000, u32::MAX))
        .with_reclaim_capacity(1);
    w.write(&"warmup message").unwrap();
    let n = count_allocs(|| {
        for _ in 0..100 {
            w.write(&"steady state msg").unwrap();
        }
    });
    assert_eq!(
        n, 0,
        "policy-installed steady-state writes must not allocate"
    );

    // Reader: warm to high-water, rewind, measure.
    let mut out = Vec::new();
    {
        let mut sw = StreamWriter::new(Cursor::new(&mut out), DefaultFramer);
        for _ in 0..10 {
            sw.write(&"payload").unwrap();
        }
    }
    let mut r = StreamReader::new(Cursor::new(&out), DefaultDeframer)
        .with_memory_policy(policy::AdaptiveWatermarkPolicy::new(1_000_000, u32::MAX))
        .with_reclaim_capacity(1);
    r.process_all(|_| Ok(())).unwrap();
    r.get_mut().set_position(0);
    let n = count_allocs(|| r.process_all(|_| Ok(())).unwrap());
    assert_eq!(
        n, 0,
        "policy-installed steady-state reads must not allocate"
    );
}
