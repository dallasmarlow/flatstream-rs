//! A1 — Write-pipeline decomposition.
//!
//! # Benchmark Purpose
//!
//! Central question: in a realistic end-to-end journaling write, **what
//! fraction of the per-record cost is flatstream's?** A consumer observed a
//! large end-to-end throughput drop and attributed it to their own
//! serialization and bookkeeping rather than to the library. That attribution
//! is plausible but was never measured, and `docs/CONTRIBUTING.md` §1 forbids
//! publishing it unmeasured. This bench measures it.
//!
//! # Design: a cumulative ladder, not seven isolated micro-benches
//!
//! Isolated measurements of seven stages do not sum to the pipeline — cache
//! state, inlining, and register pressure all differ once the stages run
//! together. So each stage here is the previous stage **plus one layer**,
//! against the identical workload and record stream. The cost of a layer is
//! the delta between two adjacent rungs, measured in situ:
//!
//! | Rung | Adds | Whose cost |
//! |------|------|------------|
//! | `s1_harvest`          | pull a pty chunk, stamp sequence + monotonic time | application |
//! | `s2_build`            | + build the FlatBuffer into a reused builder      | application (FlatBuffers) |
//! | `s3_frame`            | + `DefaultFramer` into a reused `Vec`             | **flatstream** |
//! | `s4_frame_crc32`      | + CRC-32 over the payload (`ChecksumFramer`)      | **flatstream** |
//! | `s5_buffered_file`    | + sink becomes `BufWriter<File>` instead of `Vec` | OS / libstd |
//! | `s6_index`            | + external offset index from `FrameReceipt`       | application |
//! | `s7_fsync`            | + `flush()` + `sync_data()` once per batch        | durability |
//!
//! Two cross-checks guard against the ladder lying: `x_crc32_only` times the
//! CRC-32 in isolation over the same payloads (it should agree with s4 − s3),
//! and `x_frame_sink` frames into `io::sink()` (which discards without a
//! memcpy) to separate framing's *call* overhead from the payload copy the
//! `Vec` rung includes.
//!
//! # Reading the output
//!
//! Criterion reports per-element time because `Throughput::Elements` is set to
//! the record count, so each number is **nanoseconds per record** directly.
//!
//! Run with:
//! ```text
//! cargo bench --features crc32 --bench write_pipeline_decomposition
//! ```

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use flatbuffers::FlatBufferBuilder;
use flatstream::{ChecksumFramer, Crc32, DefaultFramer, FrameReceipt, StreamWriter};
use std::io::{BufWriter, Seek, SeekFrom};
use std::time::Instant;

/// Records per iteration. Large enough to amortize the per-iteration setup
/// (one `seek`, one writer construction) into the noise, small enough that the
/// scratch file stays in page cache.
const RECORDS: usize = 1_000;

/// Chunk payload sizes. 64 B is a typical single-line terminal write; 4 KiB is
/// a screen repaint or a burst of program output.
const CHUNK_SIZES: [usize; 2] = [64, 4096];

// vtable slot offsets for the TerminalChunk table (see ONBOARDING.md §7):
// field i lives at slot offset 4 + 2*i.
const V_SEQUENCE: u16 = 4;
const V_TIMESTAMP: u16 = 6;
const V_CHANNEL: u16 = 8;
const V_DATA: u16 = 10;

/// One harvested terminal chunk, before it becomes a FlatBuffer. `data`
/// borrows the source ring — the application does not copy here, and neither
/// does anything downstream until the builder packs it.
#[derive(Clone, Copy)]
struct Chunk<'a> {
    sequence: u64,
    timestamp_nanos: u64,
    channel: u8,
    data: &'a [u8],
}

/// The application's own pre-serialization work: take the next slice of pty
/// output, stamp it with a sequence number and a monotonic timestamp. The
/// timestamp is deliberately a real `Instant::elapsed()` — a journal that
/// cannot order its records is not a journal, so this cost is intrinsic to the
/// workload rather than benchmark scaffolding.
struct Harvester<'a> {
    source: &'a [u8],
    chunk_len: usize,
    cursor: usize,
    sequence: u64,
    origin: Instant,
}

impl<'a> Harvester<'a> {
    fn new(source: &'a [u8], chunk_len: usize) -> Self {
        Self {
            source,
            chunk_len,
            cursor: 0,
            sequence: 0,
            origin: Instant::now(),
        }
    }

    #[inline]
    fn harvest(&mut self) -> Chunk<'a> {
        if self.cursor + self.chunk_len > self.source.len() {
            self.cursor = 0;
        }
        let data = &self.source[self.cursor..self.cursor + self.chunk_len];
        self.cursor += self.chunk_len;
        self.sequence += 1;
        Chunk {
            sequence: self.sequence,
            timestamp_nanos: self.origin.elapsed().as_nanos() as u64,
            channel: 1, // stdout
            data,
        }
    }
}

/// Builds the `TerminalChunk` table into a reused builder. This is the
/// application's serialization step: flatstream never builds a FlatBuffer, it
/// only frames one that already exists.
#[inline]
fn build(builder: &mut FlatBufferBuilder, chunk: &Chunk<'_>) {
    builder.reset();
    let data = builder.create_vector(chunk.data);
    let table = builder.start_table();
    builder.push_slot::<u64>(V_SEQUENCE, chunk.sequence, 0);
    builder.push_slot::<u64>(V_TIMESTAMP, chunk.timestamp_nanos, 0);
    builder.push_slot::<u8>(V_CHANNEL, chunk.channel, 0);
    builder.push_slot_always(V_DATA, data);
    let root = builder.end_table(table);
    builder.finish(root, None);
}

/// Pseudo-random but deterministic source bytes, so payload content (and
/// therefore CRC-32 work) is identical across runs and rungs.
fn source_bytes(len: usize) -> Vec<u8> {
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

fn decomposition(c: &mut Criterion) {
    let mut group = c.benchmark_group("A1 Write Pipeline Decomposition");
    // 64 KiB of source, cycled: bigger than L1 so the harvest reads are not
    // artificially free, small enough to stay resident.
    let source = source_bytes(64 * 1024);

    for chunk_len in CHUNK_SIZES {
        let id = |stage: &str| BenchmarkId::new(stage, format!("{chunk_len}B"));
        // Per-element timing: every reported number is nanoseconds per record.
        group.throughput(Throughput::Elements(RECORDS as u64));

        // --- s1: application harvest only -------------------------------------
        group.bench_function(id("s1_harvest"), |b| {
            b.iter(|| {
                let mut h = Harvester::new(black_box(&source), chunk_len);
                for _ in 0..RECORDS {
                    black_box(h.harvest());
                }
            });
        });

        // --- s2: + FlatBuffers build into a reused builder ---------------------
        group.bench_function(id("s2_build"), |b| {
            let mut builder = FlatBufferBuilder::new();
            b.iter(|| {
                let mut h = Harvester::new(black_box(&source), chunk_len);
                for _ in 0..RECORDS {
                    build(&mut builder, &h.harvest());
                    black_box(builder.finished_data());
                }
            });
        });

        // --- s3: + flatstream framing into a reused Vec ------------------------
        group.bench_function(id("s3_frame"), |b| {
            let mut builder = FlatBufferBuilder::new();
            let mut sink: Vec<u8> = Vec::with_capacity(RECORDS * (chunk_len + 64));
            b.iter(|| {
                sink.clear();
                let mut h = Harvester::new(black_box(&source), chunk_len);
                {
                    let mut writer = StreamWriter::new(&mut sink, DefaultFramer);
                    for _ in 0..RECORDS {
                        build(&mut builder, &h.harvest());
                        writer.write_finished(&mut builder).unwrap();
                    }
                }
                black_box(&sink);
            });
        });

        // --- s4: + CRC-32 over the payload -------------------------------------
        group.bench_function(id("s4_frame_crc32"), |b| {
            let mut builder = FlatBufferBuilder::new();
            let mut sink: Vec<u8> = Vec::with_capacity(RECORDS * (chunk_len + 64));
            b.iter(|| {
                sink.clear();
                let mut h = Harvester::new(black_box(&source), chunk_len);
                {
                    let mut writer =
                        StreamWriter::new(&mut sink, ChecksumFramer::new(Crc32::new()));
                    for _ in 0..RECORDS {
                        build(&mut builder, &h.harvest());
                        writer.write_finished(&mut builder).unwrap();
                    }
                }
                black_box(&sink);
            });
        });

        // --- s5: + the sink becomes a buffered real file -----------------------
        // The scratch file is created once and rewound per iteration, so the
        // measurement is steady-state append cost into page cache, not file
        // creation. No fsync here — that is s7.
        group.bench_function(id("s5_buffered_file"), |b| {
            let file = tempfile::tempfile().expect("scratch file");
            let mut builder = FlatBufferBuilder::new();
            b.iter(|| {
                (&file).seek(SeekFrom::Start(0)).unwrap();
                let mut h = Harvester::new(black_box(&source), chunk_len);
                let mut writer =
                    StreamWriter::new(BufWriter::new(&file), ChecksumFramer::new(Crc32::new()));
                for _ in 0..RECORDS {
                    build(&mut builder, &h.harvest());
                    writer.write_finished(&mut builder).unwrap();
                }
                writer.flush().unwrap();
            });
        });

        // --- s6: + external offset index from receipts (full pipeline) ---------
        group.bench_function(id("s6_index"), |b| {
            let file = tempfile::tempfile().expect("scratch file");
            let mut builder = FlatBufferBuilder::new();
            let mut index: Vec<(u64, FrameReceipt)> = Vec::with_capacity(RECORDS);
            b.iter(|| {
                (&file).seek(SeekFrom::Start(0)).unwrap();
                index.clear();
                let mut h = Harvester::new(black_box(&source), chunk_len);
                let mut writer =
                    StreamWriter::new(BufWriter::new(&file), ChecksumFramer::new(Crc32::new()));
                for _ in 0..RECORDS {
                    let chunk = h.harvest();
                    build(&mut builder, &chunk);
                    let receipt = writer.write_finished_with_receipt(&mut builder).unwrap();
                    index.push((chunk.sequence, receipt));
                }
                writer.flush().unwrap();
                black_box(&index);
            });
        });

        // --- s7: + durability, one fsync per batch -----------------------------
        // The honest end of the ladder: what the application actually pays if
        // it wants the batch to survive a power loss.
        group.bench_function(id("s7_fsync"), |b| {
            let file = tempfile::tempfile().expect("scratch file");
            let mut builder = FlatBufferBuilder::new();
            let mut index: Vec<(u64, FrameReceipt)> = Vec::with_capacity(RECORDS);
            b.iter(|| {
                (&file).seek(SeekFrom::Start(0)).unwrap();
                index.clear();
                let mut h = Harvester::new(black_box(&source), chunk_len);
                let mut writer =
                    StreamWriter::new(BufWriter::new(&file), ChecksumFramer::new(Crc32::new()));
                for _ in 0..RECORDS {
                    let chunk = h.harvest();
                    build(&mut builder, &chunk);
                    let receipt = writer.write_finished_with_receipt(&mut builder).unwrap();
                    index.push((chunk.sequence, receipt));
                }
                writer.flush().unwrap();
                writer.get_ref().get_ref().sync_data().unwrap();
                black_box(&index);
            });
        });

        // --- cross-check: CRC-32 alone over the same payloads ------------------
        // Should agree with (s4 − s3). If it does not, the ladder is measuring
        // something other than what it claims.
        group.bench_function(id("x_crc32_only"), |b| {
            use flatstream::checksum::Checksum;
            let mut builder = FlatBufferBuilder::new();
            let alg = Crc32::new();
            b.iter(|| {
                let mut h = Harvester::new(black_box(&source), chunk_len);
                for _ in 0..RECORDS {
                    build(&mut builder, &h.harvest());
                    black_box(alg.calculate(builder.finished_data()));
                }
            });
        });

        // --- cross-check: framing into a discarding sink -----------------------
        // `io::sink()` accepts and drops bytes, so this is framing's call and
        // header-assembly overhead with the payload memcpy removed. The gap to
        // s3 is the copy into the Vec.
        group.bench_function(id("x_frame_sink"), |b| {
            let mut builder = FlatBufferBuilder::new();
            b.iter(|| {
                let mut h = Harvester::new(black_box(&source), chunk_len);
                let mut writer = StreamWriter::new(std::io::sink(), DefaultFramer);
                for _ in 0..RECORDS {
                    build(&mut builder, &h.harvest());
                    writer.write_finished(&mut builder).unwrap();
                }
            });
        });
    }

    group.finish();
}

criterion_group!(benches, decomposition);
criterion_main!(benches);
