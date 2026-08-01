//! E1 — Single-`writev` framing versus the two-`write_all` path.
//!
//! # Benchmark Purpose
//!
//! `DefaultFramer` and `ChecksumFramer<Crc32>` now emit each frame's assembled
//! header and payload in **one** `write_vectored` call instead of the pre-E1
//! pair of `write_all`s. The claim to test is narrow and mechanical: on a sink
//! that implements `writev` this halves the syscalls per frame, so it should
//! win on **unbuffered** sinks and do nothing measurable on a `BufWriter`
//! (already memcpy-batched). It is a call-count win, not a copy win —
//! zero-copy already held.
//!
//! # Design: both arms in one process
//!
//! The baselines are `TwoCallFramer` and, with the `crc32` feature,
//! `TwoCallCrc32Framer`: replicas of the corresponding pre-E1 bodies,
//! including their length guards. Every `default` or `crc32` pair uses the
//! same payload and sink instance; only the framing write shape differs.
//! Running both arms in a single binary means one build, one machine, one
//! thermal state — strictly better evidence than comparing two Criterion
//! baselines across a code change, and immune to the "baselines are
//! machine-local and die with `cargo clean`" caveat in the project benchmark
//! evidence rules.
//!
//! Both arms go through `StreamWriter`, so the vectored arm also pays for
//! `CountingWriter::write_vectored` — the receipt accounting is inside the
//! measurement, not excluded from it.
//!
//! The payload is built once and never rebuilt: `write_finished` re-reads the
//! same finished builder every record, so FlatBuffers construction contributes
//! nothing and the delta is purely framing plus sink.
//!
//! # Sinks
//!
//! - `file` — a raw `File`, unbuffered. Every frame is a real syscall (two
//!   before, one now). This is where the win must show up if it exists.
//! - `tcp` — a loopback `TcpStream` with `TCP_NODELAY`, drained by a
//!   background thread. Same syscall argument, different kernel path.
//! - `bufwriter` — `BufWriter<File>`. Expected to be a wash: `BufWriter`
//!   implements `write_vectored` by copying every slice into its buffer, which
//!   is what the two-call path already did.
//!
//! Run with:
//! ```text
//! cargo bench --bench vectored_framing
//! cargo bench --bench vectored_framing --features crc32  # includes CRC32 pair
//! ```

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use flatbuffers::FlatBufferBuilder;
#[cfg(feature = "crc32")]
use flatstream::{checksum::Checksum, ChecksumFramer, Crc32};
use flatstream::{DefaultFramer, Error, Framer, Result, StreamWriter};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};

/// Frames per iteration.
const RECORDS: usize = 1_000;

/// Payload sizes: a small terminal chunk, and a page-sized record.
const PAYLOAD_SIZES: [usize; 2] = [64, 4096];

/// The pre-E1 `DefaultFramer`, verbatim: length prefix and payload as two
/// separate `write_all` calls. Kept here (not in the library) purely as the
/// measurement baseline.
struct TwoCallFramer;

impl Framer for TwoCallFramer {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        if payload.len() > u32::MAX as usize {
            return Err(Error::invalid_frame_with(
                "payload length exceeds 32-bit header limit",
                Some(payload.len()),
                None,
                Some(u32::MAX as usize),
            ));
        }
        let payload_len = payload.len() as u32;
        writer.write_all(&payload_len.to_le_bytes())?;
        writer.write_all(payload)?;
        Ok(())
    }
}

/// The pre-E1 `ChecksumFramer<Crc32>`: one write for its already-assembled
/// `[length | checksum]` header and one for the payload.
#[cfg(feature = "crc32")]
struct TwoCallCrc32Framer {
    checksum_alg: Crc32,
}

#[cfg(feature = "crc32")]
impl TwoCallCrc32Framer {
    fn new() -> Self {
        Self {
            checksum_alg: Crc32::new(),
        }
    }
}

#[cfg(feature = "crc32")]
impl Framer for TwoCallCrc32Framer {
    fn frame_and_write<W: Write>(&self, writer: &mut W, payload: &[u8]) -> Result<()> {
        if payload.len() > u32::MAX as usize {
            return Err(Error::invalid_frame_with(
                "payload length exceeds 32-bit header limit",
                Some(payload.len()),
                None,
                Some(u32::MAX as usize),
            ));
        }
        let payload_len = payload.len() as u32;
        let checksum = self.checksum_alg.calculate(payload);

        let mut header = [0u8; 12];
        header[..4].copy_from_slice(&payload_len.to_le_bytes());
        let checksum_field: &mut [u8; 8] = (&mut header[4..12]).try_into().unwrap();
        self.checksum_alg.write_bytes(checksum, checksum_field);

        writer.write_all(&header[..4 + Crc32::SIZE])?;
        writer.write_all(payload)?;
        Ok(())
    }
}

/// A builder holding one finished payload of `size` bytes, reused for every
/// record so that no FlatBuffers construction cost enters the measurement.
fn finished_builder(size: usize) -> FlatBufferBuilder<'static> {
    let mut builder = FlatBufferBuilder::new();
    let bytes = vec![0xA5u8; size];
    let v = builder.create_vector(&bytes);
    builder.finish(v, None);
    builder
}

/// A loopback TCP connection whose peer is continuously drained, plus the
/// join handle for the drain thread. Dropping the client closes the
/// connection and lets the drain thread finish.
struct Loopback {
    client: TcpStream,
    drain: Option<std::thread::JoinHandle<()>>,
}

impl Loopback {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let drain = std::thread::spawn(move || {
            let (mut peer, _) = listener.accept().expect("accept");
            let mut sink = vec![0u8; 256 * 1024];
            // Read until the client hangs up.
            while peer.read(&mut sink).unwrap_or(0) > 0 {}
        });
        let client = TcpStream::connect(addr).expect("connect loopback");
        // Without this, Nagle batches small frames and we would be measuring
        // the kernel's coalescing rather than our call count.
        client.set_nodelay(true).expect("nodelay");
        Self {
            client,
            drain: Some(drain),
        }
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        let _ = self.client.shutdown(std::net::Shutdown::Both);
        if let Some(h) = self.drain.take() {
            let _ = h.join();
        }
    }
}

/// Drives `RECORDS` frames through a `StreamWriter` over `sink` with `framer`.
fn drive<W: Write, F: Framer>(sink: W, framer: F, builder: &mut FlatBufferBuilder) {
    let mut writer = StreamWriter::new(sink, framer);
    for _ in 0..RECORDS {
        writer.write_finished(builder).unwrap();
    }
    writer.flush().unwrap();
    black_box(writer.bytes_written());
}

fn vectored_framing(c: &mut Criterion) {
    let mut group = c.benchmark_group("E1 Vectored Framing");
    group.throughput(Throughput::Elements(RECORDS as u64));

    for size in PAYLOAD_SIZES {
        let mut builder = finished_builder(size);

        // Each adjacent label pair shares this File and payload; only the
        // framing write shape differs.
        {
            let file = tempfile::tempfile().expect("scratch file");
            group.bench_function(
                BenchmarkId::new("file/default/vectored", format!("{size}B")),
                |b| {
                    b.iter(|| {
                        (&file).seek(SeekFrom::Start(0)).unwrap();
                        drive(&file, DefaultFramer, &mut builder);
                    });
                },
            );
            group.bench_function(
                BenchmarkId::new("file/default/two_call", format!("{size}B")),
                |b| {
                    b.iter(|| {
                        (&file).seek(SeekFrom::Start(0)).unwrap();
                        drive(&file, TwoCallFramer, &mut builder);
                    });
                },
            );
            #[cfg(feature = "crc32")]
            group.bench_function(
                BenchmarkId::new("file/crc32/vectored", format!("{size}B")),
                |b| {
                    b.iter(|| {
                        (&file).seek(SeekFrom::Start(0)).unwrap();
                        drive(&file, ChecksumFramer::new(Crc32::new()), &mut builder);
                    });
                },
            );
            #[cfg(feature = "crc32")]
            group.bench_function(
                BenchmarkId::new("file/crc32/two_call", format!("{size}B")),
                |b| {
                    b.iter(|| {
                        (&file).seek(SeekFrom::Start(0)).unwrap();
                        drive(&file, TwoCallCrc32Framer::new(), &mut builder);
                    });
                },
            );
        }

        // Each adjacent label pair shares this loopback connection and payload.
        {
            let link = Loopback::new();
            group.bench_function(
                BenchmarkId::new("tcp/default/vectored", format!("{size}B")),
                |b| b.iter(|| drive(&link.client, DefaultFramer, &mut builder)),
            );
            group.bench_function(
                BenchmarkId::new("tcp/default/two_call", format!("{size}B")),
                |b| b.iter(|| drive(&link.client, TwoCallFramer, &mut builder)),
            );
            #[cfg(feature = "crc32")]
            group.bench_function(
                BenchmarkId::new("tcp/crc32/vectored", format!("{size}B")),
                |b| {
                    b.iter(|| {
                        drive(
                            &link.client,
                            ChecksumFramer::new(Crc32::new()),
                            &mut builder,
                        )
                    })
                },
            );
            #[cfg(feature = "crc32")]
            group.bench_function(
                BenchmarkId::new("tcp/crc32/two_call", format!("{size}B")),
                |b| b.iter(|| drive(&link.client, TwoCallCrc32Framer::new(), &mut builder)),
            );
        }

        // Each adjacent label pair shares this File and payload; BufWriter is
        // expected to make both write shapes equivalent.
        {
            let file = tempfile::tempfile().expect("scratch file");
            group.bench_function(
                BenchmarkId::new("bufwriter/default/vectored", format!("{size}B")),
                |b| {
                    b.iter(|| {
                        (&file).seek(SeekFrom::Start(0)).unwrap();
                        drive(BufWriter::new(&file), DefaultFramer, &mut builder);
                    });
                },
            );
            group.bench_function(
                BenchmarkId::new("bufwriter/default/two_call", format!("{size}B")),
                |b| {
                    b.iter(|| {
                        (&file).seek(SeekFrom::Start(0)).unwrap();
                        drive(BufWriter::new(&file), TwoCallFramer, &mut builder);
                    });
                },
            );
            #[cfg(feature = "crc32")]
            group.bench_function(
                BenchmarkId::new("bufwriter/crc32/vectored", format!("{size}B")),
                |b| {
                    b.iter(|| {
                        (&file).seek(SeekFrom::Start(0)).unwrap();
                        drive(
                            BufWriter::new(&file),
                            ChecksumFramer::new(Crc32::new()),
                            &mut builder,
                        );
                    });
                },
            );
            #[cfg(feature = "crc32")]
            group.bench_function(
                BenchmarkId::new("bufwriter/crc32/two_call", format!("{size}B")),
                |b| {
                    b.iter(|| {
                        (&file).seek(SeekFrom::Start(0)).unwrap();
                        drive(
                            BufWriter::new(&file),
                            TwoCallCrc32Framer::new(),
                            &mut builder,
                        );
                    });
                },
            );
        }
    }

    group.finish();
}

criterion_group!(benches, vectored_framing);
criterion_main!(benches);
