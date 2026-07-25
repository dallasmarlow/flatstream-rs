//! Static durability-policy overhead and real checkpoint cadence.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use flatbuffers::FlatBufferBuilder;
use flatstream::{DefaultFramer, Durable, StreamWriter, SyncEveryNFrames, SyncMode};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::num::NonZeroU64;

const DISPATCH_RECORDS: usize = 1_000;
const FILE_RECORDS: usize = 100;

struct DurableVec {
    bytes: Vec<u8>,
    syncs: usize,
}

impl DurableVec {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(capacity),
            syncs: 0,
        }
    }
}

impl Write for DurableVec {
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

impl Durable for DurableVec {
    fn sync_data(&mut self) -> io::Result<()> {
        self.syncs += 1;
        Ok(())
    }

    fn sync_all(&mut self) -> io::Result<()> {
        self.syncs += 1;
        Ok(())
    }
}

fn finished_builder() -> FlatBufferBuilder<'static> {
    let mut builder = FlatBufferBuilder::new();
    let payload = builder.create_vector(&[0xA5u8; 64]);
    builder.finish(payload, None);
    builder
}

fn sync_policy_dispatch(c: &mut Criterion) {
    let mut group = c.benchmark_group("Sync Policy Dispatch");
    group.throughput(Throughput::Elements(DISPATCH_RECORDS as u64));
    let mut builder = finished_builder();

    group.bench_function("default_no_sync", |b| {
        b.iter(|| {
            let sink = DurableVec::with_capacity(DISPATCH_RECORDS * 128);
            let mut writer = StreamWriter::new(sink, DefaultFramer);
            for _ in 0..DISPATCH_RECORDS {
                writer.write_finished(&mut builder).unwrap();
            }
            black_box(writer.into_inner().bytes.len());
        });
    });

    group.bench_function("static_policy_not_due", |b| {
        b.iter(|| {
            let sink = DurableVec::with_capacity(DISPATCH_RECORDS * 128);
            let policy = SyncEveryNFrames::new(
                NonZeroU64::new((DISPATCH_RECORDS + 1) as u64).unwrap(),
                SyncMode::Data,
            );
            let mut writer = StreamWriter::new(sink, DefaultFramer).with_sync_policy(policy);
            for _ in 0..DISPATCH_RECORDS {
                writer.write_finished(&mut builder).unwrap();
            }
            let sink = writer.into_inner();
            black_box((sink.bytes.len(), sink.syncs));
        });
    });

    group.bench_function("static_policy_one_checkpoint", |b| {
        b.iter(|| {
            let sink = DurableVec::with_capacity(DISPATCH_RECORDS * 128);
            let policy = SyncEveryNFrames::new(
                NonZeroU64::new(DISPATCH_RECORDS as u64).unwrap(),
                SyncMode::Data,
            );
            let mut writer = StreamWriter::new(sink, DefaultFramer).with_sync_policy(policy);
            for _ in 0..DISPATCH_RECORDS {
                writer.write_finished(&mut builder).unwrap();
            }
            let sink = writer.into_inner();
            black_box((sink.bytes.len(), sink.syncs));
        });
    });

    group.finish();
}

fn sync_policy_file_cadence(c: &mut Criterion) {
    let mut group = c.benchmark_group("Sync Policy File Cadence");
    group.throughput(Throughput::Elements(FILE_RECORDS as u64));
    let mut builder = finished_builder();

    let cadences = [1u64, 10, FILE_RECORDS as u64];
    for cadence in cadences {
        let mut file = tempfile::tempfile().expect("scratch file");
        group.bench_function(BenchmarkId::new("sync_data_every", cadence), |b| {
            b.iter(|| {
                file.set_len(0).unwrap();
                file.seek(SeekFrom::Start(0)).unwrap();
                let policy =
                    SyncEveryNFrames::new(NonZeroU64::new(cadence).unwrap(), SyncMode::Data);
                let mut writer = StreamWriter::new(BufWriter::new(&mut file), DefaultFramer)
                    .with_sync_policy(policy);
                for _ in 0..FILE_RECORDS {
                    writer.write_finished(&mut builder).unwrap();
                }
                if writer.durable_watermark() != Some(writer.bytes_written()) {
                    writer.sync_data().unwrap();
                }
                black_box(writer.durable_watermark());
            });
        });
    }

    group.finish();
}

criterion_group!(benches, sync_policy_dispatch, sync_policy_file_cadence);
criterion_main!(benches);
