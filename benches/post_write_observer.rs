//! B3 — installed post-write observer overhead.
//!
//! The pair differs only by the writer's final generic observer state. The
//! installed arm consumes receipt, payload-length, and elapsed-time fields so
//! LLVM cannot erase the clock reads or callback.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use flatbuffers::FlatBufferBuilder;
use flatstream::{DefaultFramer, PostWriteEvent, PostWriteOutcome, StreamWriter};
use std::io::{self, Write};

const FRAMES: usize = 1_000;

struct ReusedVec {
    bytes: Vec<u8>,
}

impl ReusedVec {
    fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(FRAMES * 128),
        }
    }
}

impl Write for ReusedVec {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn write_vectored(&mut self, bufs: &[io::IoSlice<'_>]) -> io::Result<usize> {
        let mut total = 0;
        for buf in bufs {
            self.bytes.extend_from_slice(buf);
            total += buf.len();
        }
        Ok(total)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn finished_builder() -> FlatBufferBuilder<'static> {
    let mut builder = FlatBufferBuilder::new();
    let payload = builder.create_vector(&[0xA5u8; 64]);
    builder.finish(payload, None);
    builder
}

fn post_write_observer(c: &mut Criterion) {
    let mut group = c.benchmark_group("B3 Post Write Observer");
    group.throughput(Throughput::Elements(FRAMES as u64));
    let mut builder = finished_builder();

    group.bench_function("default_no_observer", |b| {
        b.iter(|| {
            let mut writer = StreamWriter::new(ReusedVec::new(), DefaultFramer);
            for _ in 0..FRAMES {
                writer.write_finished(&mut builder).unwrap();
            }
            black_box(writer.into_inner().bytes.len());
        });
    });

    group.bench_function("installed_receipt_and_latency", |b| {
        b.iter(|| {
            let mut observed = 0usize;
            let mut writer = StreamWriter::new(ReusedVec::new(), DefaultFramer)
                .with_post_write_observer(|event: PostWriteEvent<'_>| {
                    let receipt = match event.outcome {
                        PostWriteOutcome::Succeeded(receipt) => receipt,
                        other => panic!("in-memory sink unexpectedly failed: {other:?}"),
                    };
                    black_box((event.payload_len, event.elapsed, receipt));
                    observed += 1;
                });
            for _ in 0..FRAMES {
                writer.write_finished(&mut builder).unwrap();
            }
            let bytes = writer.into_inner().bytes.len();
            black_box((bytes, observed));
        });
    });

    group.finish();
}

criterion_group!(benches, post_write_observer);
criterion_main!(benches);
