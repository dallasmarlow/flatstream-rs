#![cfg_attr(miri, ignore)]
// Minimal counting allocator using std::alloc hooks (test-only heuristic)
use flatstream::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

struct CountingAlloc;
static ALLOC_COUNT: AtomicUsize = AtomicUsize::new(0);
static ENABLED: OnceLock<bool> = OnceLock::new();

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if *ENABLED.get_or_init(|| false) {
            ALLOC_COUNT.fetch_add(1, Ordering::Relaxed);
        }
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static GA: CountingAlloc = CountingAlloc;

fn reset_alloc_counter() {
    ALLOC_COUNT.store(0, Ordering::Relaxed);
}
fn get_allocs() -> usize {
    ALLOC_COUNT.load(Ordering::Relaxed)
}

#[test]
fn zero_alloc_in_process_all_and_messages() {
    // Build a stream with multiple messages
    let mut out = Vec::new();
    for _ in 0..5 {
        DefaultFramer.frame_and_write(&mut out, b"x").unwrap();
    }

    // Enable counting now to exclude setup allocations
    let _ = ENABLED.set(true);

    // process_all path
    reset_alloc_counter();
    {
        let mut r = StreamReader::new(std::io::Cursor::new(&out), DefaultDeframer);
        r.process_all(|_| Ok(())).unwrap();
    }
    assert_eq!(get_allocs(), 0, "process_all should not allocate");

    // messages() path
    reset_alloc_counter();
    {
        let mut r = StreamReader::new(std::io::Cursor::new(&out), DefaultDeframer);
        let mut it = r.messages();
        while let Some(_p) = it.next().unwrap() {}
    }
    assert_eq!(get_allocs(), 0, "messages() should not allocate");
}
