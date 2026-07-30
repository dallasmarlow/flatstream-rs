//! A3 — Read-path copy cost.
//!
//! # Benchmark Purpose
//!
//! Central question: **what fraction of a frame read is the one unavoidable
//! copy?** Reading a frame from a generic [`Read`](std::io::Read) source copies
//! its payload exactly once, from the source into the reader's reusable buffer,
//! before the payload is handed out as a borrowed `&[u8]`. That copy is the
//! single place the current design is not copy-free, and the README TL;DR and
//! `docs/DESIGN_v2_7.md` §1 both name a borrowed-slice/mmap source — which would
//! remove it — as future work. This bench measures the ceiling that future
//! source could reclaim: the copy's share of read time, across frame sizes and
//! for default and CRC-32 framing.
//!
//! # Design: an A/B against a model of the future borrowed source
//!
//! The copy lives inside `read_payload` (`src/framing.rs`): a single
//! `read_exact(&mut buffer[..len])`. It cannot be removed through the `Read`
//! trait — a borrowed-slice source is a *different* API that hands out
//! `&source[..]` instead of filling a buffer — so this bench compares the real
//! read against a hand-written model of that not-yet-built API. Three arms per
//! (framing, size), all over an identical in-memory wire and reading identical
//! payload bytes:
//!
//! | Arm | Does | Copies payload? |
//! |-----|------|-----------------|
//! | `read_copy`    | the real `Deframer::read_and_deframe` into a reused buffer | **yes** — the memcpy under test |
//! | `borrow_slice` | identical header parse + (CRC verify), then borrows `&wire[..]` | no |
//! | `memcpy_only`  | cross-check: raw `copy_from_slice` of the payload bytes only | yes |
//!
//! The copy's per-frame cost is `read_copy − borrow_slice`; its fraction of read
//! time is `(read_copy − borrow_slice) / read_copy`. The `borrow_slice` arm does
//! the *same* checksum verification as `read_copy` (over the borrowed bytes for
//! CRC-32; a no-op for default), so the delta is the memcpy and not the CRC pass.
//! `memcpy_only` is the ladder's honesty check, exactly as `x_crc32_only` guards
//! the A1 write ladder: `read_copy − borrow_slice` should track a raw memcpy of
//! `payload_len` bytes. If it does not, the A/B is measuring something other than
//! the copy.
//!
//! The default and CRC-32 arms answer a second question the fraction depends on:
//! with CRC-32 the verify adds a second O(payload) pass, so the copy is a smaller
//! *fraction* of read time even though its absolute cost is unchanged.
//!
//! # An in-memory source is deliberate
//!
//! The source is an in-memory `&[u8]` (the leanest `Read`, so `read_copy`'s
//! non-copy overhead is minimal and the memcpy stands out). This isolates the
//! copy from file I/O and syscalls on purpose: A3 quantifies the *copy*, not the
//! source. Real-file read time adds I/O the copy fraction here does not model —
//! see the findings doc's threats-to-validity.
//!
//! # Reading the output
//!
//! `Throughput::Elements` is the frame count, so every reported number is
//! **nanoseconds per frame** directly.
//!
//! Run with:
//! ```text
//! cargo bench --features crc32 --bench read_path_copy
//! ```

use criterion::measurement::WallTime;
use criterion::{
    black_box, criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};
use flatstream::checksum::Checksum;
use flatstream::{
    ChecksumDeframer, ChecksumFramer, Crc32, DefaultDeframer, DefaultFramer, Deframer, Framer,
    NoChecksum,
};

/// Payload sizes: 64 B (single terminal line), 4 KiB (screen repaint — the
/// canonical size used across the repo's benches), 64 KiB, and 256 KiB. The
/// spread is the point: the copy is O(payload) while header parse and loop
/// overhead are O(1), so the copy's fraction of read time is expected to climb
/// with size.
const PAYLOAD_SIZES: [usize; 4] = [64, 4096, 64 * 1024, 256 * 1024];

/// Per-size frame counts are chosen so every arm reads roughly the same total
/// wire (~8 MiB), floored so the small sizes still amortize per-iteration setup.
/// Equal working sets keep cache behavior comparable across sizes rather than
/// letting the byte total confound the per-frame number.
const TARGET_WIRE_BYTES: usize = 8 * 1024 * 1024;
const MIN_FRAMES: usize = 64;

fn frame_count(payload_size: usize) -> usize {
    (TARGET_WIRE_BYTES / (payload_size + 16)).max(MIN_FRAMES)
}

/// Deterministic pseudo-random payload bytes (xorshift64), so payload content —
/// and therefore CRC-32 work — is identical across runs and arms, and no memcpy
/// or checksum path can special-case an all-zero buffer.
fn payload_bytes(len: usize) -> Vec<u8> {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 33) as u8
        })
        .collect()
}

/// Frames `frames` copies of `payload` with `framer` into one contiguous wire —
/// byte-identical to what a `StreamWriter` over the same framer would produce.
fn build_wire<F: Framer>(framer: &F, payload: &[u8], frames: usize) -> Vec<u8> {
    let mut wire = Vec::new();
    for _ in 0..frames {
        framer.frame_and_write(&mut wire, payload).unwrap();
    }
    wire
}

/// Emits the three arms for one framing scheme. `checksum` drives the
/// `borrow_slice` arm's verification and, through `C::SIZE`, the header width:
/// `NoChecksum` (`SIZE == 0`) collapses the checksum field and verify to nothing,
/// exactly matching the default deframer, while `Crc32` (`SIZE == 4`) reproduces
/// the checksummed header and the payload-covering verify.
fn bench_framing<F, D, C>(
    group: &mut BenchmarkGroup<'_, WallTime>,
    framing: &str,
    framer: F,
    deframer: D,
    checksum: C,
) where
    F: Framer,
    D: Deframer,
    C: Checksum + Copy,
{
    // Header width on the wire: 4-byte length prefix plus the checksum field.
    let header_len = 4 + C::SIZE;

    for size in PAYLOAD_SIZES {
        let frames = frame_count(size);
        let payload = payload_bytes(size);
        let wire = build_wire(&framer, &payload, frames);
        // Per-frame timing: every reported number is nanoseconds per frame.
        group.throughput(Throughput::Elements(frames as u64));
        let id = |arm: &str| BenchmarkId::new(arm, format!("{framing}/{size}B"));

        // --- read_copy: the real read path, payload copied into a reused buffer.
        // Pre-sized to the payload so no growth or reallocation happens during
        // measurement (the warmed high-water-mark steady state the invariants
        // describe); the copy is then the only per-frame O(payload) memory write.
        let mut buffer = vec![0u8; size];
        group.bench_function(id("read_copy"), |b| {
            b.iter(|| {
                let mut src: &[u8] = black_box(&wire[..]);
                let mut n = 0usize;
                while let Some(len) = deframer.read_and_deframe(&mut src, &mut buffer).unwrap() {
                    black_box(&buffer[..len]);
                    n += 1;
                }
                black_box(n)
            });
        });

        // --- borrow_slice: model of the future borrowed-slice/mmap source. Same
        // header parse and (for CRC-32) the same payload-covering verify, but the
        // payload is handed out as a borrow of the wire — no copy into a buffer.
        group.bench_function(id("borrow_slice"), |b| {
            b.iter(|| {
                let wire = black_box(&wire[..]);
                let mut pos = 0usize;
                let mut n = 0usize;
                while pos < wire.len() {
                    let len = u32::from_le_bytes(wire[pos..pos + 4].try_into().unwrap()) as usize;
                    let expected = checksum.read_bytes(&wire[pos + 4..pos + header_len]);
                    let payload = &wire[pos + header_len..pos + header_len + len];
                    checksum.verify(expected, payload).unwrap();
                    black_box(payload);
                    pos += header_len + len;
                    n += 1;
                }
                black_box(n)
            });
        });

        // --- memcpy_only: cross-check. Just the payload copy, no header parse and
        // no verify. `read_copy − borrow_slice` should track this raw memcpy; if
        // it does not, the A/B is capturing something other than the copy.
        let mut dst = vec![0u8; size];
        group.bench_function(id("memcpy_only"), |b| {
            b.iter(|| {
                let wire = black_box(&wire[..]);
                let mut pos = 0usize;
                let mut n = 0usize;
                while pos < wire.len() {
                    let len = u32::from_le_bytes(wire[pos..pos + 4].try_into().unwrap()) as usize;
                    let payload = &wire[pos + header_len..pos + header_len + len];
                    dst[..len].copy_from_slice(payload);
                    black_box(&dst[..len]);
                    pos += header_len + len;
                    n += 1;
                }
                black_box(n)
            });
        });
    }
}

fn read_path_copy(c: &mut Criterion) {
    let mut group = c.benchmark_group("A3 Read Path Copy");

    bench_framing(
        &mut group,
        "default",
        DefaultFramer,
        DefaultDeframer::new(),
        NoChecksum::new(),
    );
    bench_framing(
        &mut group,
        "crc32",
        ChecksumFramer::new(Crc32::new()),
        ChecksumDeframer::new(Crc32::new()),
        Crc32::new(),
    );

    group.finish();
}

criterion_group!(benches, read_path_copy);
criterion_main!(benches);
