//! A4 — compression feasibility for journal payloads.
//!
//! This is an experiment, not a wire-format proposal. It compares identity
//! access, LZ4 block compression, and Zstandard level 1 over three payload
//! size classes for two deterministic control distributions:
//!
//! - a highly compressible repeated terminal-output pattern;
//! - deterministic xorshift bytes (the incompressible control).
//!
//! Every codec context and input/output buffer is created before timing and
//! reused. The buffered-file arm is payload-only: it deliberately does not
//! invent a compressed flatstream envelope or imply proposed wire bytes.
//! Throughput is always logical (uncompressed) input bytes per second.
//!
//! Run one `(distribution, size)` group at a time. `A4_CASE` selects the
//! group before Criterion starts, avoiding cross-group machine drift:
//!
//! ```text
//! A4_CASE=compressible_4k \
//!   scripts/bench_isolated.sh a4_compressible_4k compression_feasibility '' \
//!   -- --locked
//! ```

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use lz4_flex::block::{
    compress_into_with_table as lz4_compress, decompress_into as lz4_decompress,
    get_maximum_output_size as lz4_bound, CompressTable,
};
use sha2::{Digest, Sha256};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use zstd::bulk::{Compressor as ZstdCompressor, Decompressor as ZstdDecompressor};

const ZSTD_LEVEL: i32 = 1;
const FILE_BATCH_BYTES: usize = 8 * 1024 * 1024;
const FILE_BUFFER_BYTES: usize = 64 * 1024;
const SIZE_CLASSES: [(&str, &str, usize); 3] = [
    ("4k", "4KiB", 4 * 1024),
    ("64k", "64KiB", 64 * 1024),
    ("256k", "256KiB", 256 * 1024),
];

fn lz4_table(input_len: usize) -> CompressTable {
    if input_len < u16::MAX as usize {
        CompressTable::small()
    } else {
        CompressTable::large()
    }
}

fn highly_compressible(len: usize) -> Vec<u8> {
    const PATTERN: &[u8] = b"test positioned_reads::retry_after_partial_frame ... ok\r\n";
    PATTERN.iter().copied().cycle().take(len).collect()
}

fn incompressible(len: usize) -> Vec<u8> {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    (0..len)
        .map(|_| {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            state = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
            (state >> 32) as u8
        })
        .collect()
}

struct Prepared {
    lz4: Vec<u8>,
    lz4_len: usize,
    zstd: Vec<u8>,
    zstd_len: usize,
}

impl Prepared {
    fn new(input: &[u8]) -> Self {
        let mut lz4 = vec![0u8; lz4_bound(input.len())];
        let mut table = lz4_table(input.len());
        let lz4_len = lz4_compress(input, &mut lz4, &mut table).expect("LZ4 fixture compression");

        let mut zstd = vec![0u8; zstd::zstd_safe::compress_bound(input.len())];
        let mut compressor = ZstdCompressor::new(ZSTD_LEVEL).expect("Zstd compressor");
        let zstd_len = compressor
            .compress_to_buffer(input, &mut zstd[..])
            .expect("Zstd fixture compression");

        let mut decoded = vec![0u8; input.len()];
        let decoded_len =
            lz4_decompress(&lz4[..lz4_len], &mut decoded).expect("LZ4 fixture decompression");
        assert_eq!(decoded_len, input.len());
        assert_eq!(decoded, input);

        decoded.fill(0);
        let mut decompressor = ZstdDecompressor::new().expect("Zstd decompressor");
        let decoded_len = decompressor
            .decompress_to_buffer(&zstd[..zstd_len], &mut decoded[..])
            .expect("Zstd fixture decompression");
        assert_eq!(decoded_len, input.len());
        assert_eq!(decoded, input);

        Self {
            lz4,
            lz4_len,
            zstd,
            zstd_len,
        }
    }
}

fn benchmark_case(c: &mut Criterion, distribution: &str, size: &str, input: &[u8]) {
    assert!(!input.is_empty());
    let prepared = Prepared::new(input);
    let digest = Sha256::digest(input);
    println!(
        "# A4_INPUT distribution={distribution} size={size} bytes={} sha256={digest:x} \
         lz4_bytes={} lz4_ratio={:.6} zstd1_bytes={} zstd1_ratio={:.6}",
        input.len(),
        prepared.lz4_len,
        prepared.lz4_len as f64 / input.len() as f64,
        prepared.zstd_len,
        prepared.zstd_len as f64 / input.len() as f64,
    );

    let mut group = c.benchmark_group(format!("A4 Compression/{distribution}/{size}"));
    group.throughput(Throughput::Bytes(input.len() as u64));

    // The uncompressed codec baseline is intentionally identity access:
    // flatstream already writes a borrowed payload and hands readers a borrowed
    // slice after filling the reader buffer. Copying here would charge work the
    // current codec path does not perform.
    group.bench_function("encode/uncompressed", |b| {
        b.iter(|| black_box(black_box(input).len()));
    });

    group.bench_function("encode/lz4", |b| {
        let mut output = vec![0u8; lz4_bound(input.len())];
        let mut table = lz4_table(input.len());
        lz4_compress(input, &mut output, &mut table).expect("warm LZ4 compressor");
        b.iter(|| {
            let written =
                lz4_compress(black_box(input), &mut output, &mut table).expect("LZ4 compression");
            black_box(&output[..written]);
        });
    });

    group.bench_function("encode/zstd1", |b| {
        let mut output = vec![0u8; zstd::zstd_safe::compress_bound(input.len())];
        let mut compressor = ZstdCompressor::new(ZSTD_LEVEL).expect("Zstd compressor");
        compressor
            .compress_to_buffer(input, &mut output[..])
            .expect("warm Zstd compressor");
        b.iter(|| {
            let written = compressor
                .compress_to_buffer(black_box(input), &mut output[..])
                .expect("Zstd compression");
            black_box(&output[..written]);
        });
    });

    group.bench_function("decode/uncompressed", |b| {
        b.iter(|| black_box(black_box(input).len()));
    });

    group.bench_function("decode/lz4", |b| {
        let compressed = &prepared.lz4[..prepared.lz4_len];
        let mut output = vec![0u8; input.len()];
        assert_eq!(
            lz4_decompress(compressed, &mut output).expect("warm LZ4 decompressor"),
            input.len()
        );
        b.iter(|| {
            let written =
                lz4_decompress(black_box(compressed), &mut output).expect("LZ4 decompression");
            black_box(&output[..written]);
        });
    });

    group.bench_function("decode/zstd1", |b| {
        let compressed = &prepared.zstd[..prepared.zstd_len];
        let mut output = vec![0u8; input.len()];
        let mut decompressor = ZstdDecompressor::new().expect("Zstd decompressor");
        assert_eq!(
            decompressor
                .decompress_to_buffer(compressed, &mut output[..])
                .expect("warm Zstd decompressor"),
            input.len()
        );
        b.iter(|| {
            let written = decompressor
                .decompress_to_buffer(black_box(compressed), &mut output[..])
                .expect("Zstd decompression");
            black_box(&output[..written]);
        });
    });

    let frames = (FILE_BATCH_BYTES / input.len()).max(1);
    let logical_batch_bytes = frames * input.len();
    group.throughput(Throughput::Bytes(logical_batch_bytes as u64));

    group.bench_function("buffered_file/uncompressed", |b| {
        let file = tempfile::tempfile().expect("scratch file");
        let mut writer = BufWriter::with_capacity(FILE_BUFFER_BYTES, file);
        b.iter(|| {
            writer
                .seek(SeekFrom::Start(0))
                .expect("rewind scratch file");
            for _ in 0..frames {
                writer.write_all(black_box(input)).expect("write payload");
            }
            writer.flush().expect("flush scratch file");
            black_box(logical_batch_bytes);
        });
    });

    group.bench_function("buffered_file/lz4", |b| {
        let file = tempfile::tempfile().expect("scratch file");
        let mut writer = BufWriter::with_capacity(FILE_BUFFER_BYTES, file);
        let mut output = vec![0u8; lz4_bound(input.len())];
        let mut table = lz4_table(input.len());
        lz4_compress(input, &mut output, &mut table).expect("warm LZ4 compressor");
        b.iter(|| {
            writer
                .seek(SeekFrom::Start(0))
                .expect("rewind scratch file");
            let mut stored = 0usize;
            for _ in 0..frames {
                let written = lz4_compress(black_box(input), &mut output, &mut table)
                    .expect("LZ4 compression");
                writer
                    .write_all(&output[..written])
                    .expect("write LZ4 payload");
                stored += written;
            }
            writer.flush().expect("flush scratch file");
            black_box(stored);
        });
    });

    group.bench_function("buffered_file/zstd1", |b| {
        let file = tempfile::tempfile().expect("scratch file");
        let mut writer = BufWriter::with_capacity(FILE_BUFFER_BYTES, file);
        let mut output = vec![0u8; zstd::zstd_safe::compress_bound(input.len())];
        let mut compressor = ZstdCompressor::new(ZSTD_LEVEL).expect("Zstd compressor");
        compressor
            .compress_to_buffer(input, &mut output[..])
            .expect("warm Zstd compressor");
        b.iter(|| {
            writer
                .seek(SeekFrom::Start(0))
                .expect("rewind scratch file");
            let mut stored = 0usize;
            for _ in 0..frames {
                let written = compressor
                    .compress_to_buffer(black_box(input), &mut output[..])
                    .expect("Zstd compression");
                writer
                    .write_all(&output[..written])
                    .expect("write Zstd payload");
                stored += written;
            }
            writer.flush().expect("flush scratch file");
            black_box(stored);
        });
    });

    group.finish();
}

fn compression_feasibility(c: &mut Criterion) {
    let selected = std::env::var("A4_CASE").ok();
    let should_run = |case: &str| selected.as_deref().is_none_or(|value| value == case);
    let mut cases_run = 0usize;

    for (suffix, size, bytes) in SIZE_CLASSES {
        if should_run(&format!("compressible_{suffix}")) {
            let input = highly_compressible(bytes);
            benchmark_case(c, "compressible", size, &input);
            cases_run += 1;
        }
    }

    for (suffix, size, bytes) in SIZE_CLASSES {
        if should_run(&format!("incompressible_{suffix}")) {
            let input = incompressible(bytes);
            benchmark_case(c, "incompressible", size, &input);
            cases_run += 1;
        }
    }

    assert!(
        cases_run > 0,
        "unknown A4_CASE {:?}; expected <compressible|incompressible>_<4k|64k|256k>",
        selected
    );
}

criterion_group!(benches, compression_feasibility);
criterion_main!(benches);
