//! Point lookup with caller-owned scratch versus a fresh StreamReader per read.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use flatbuffers::FlatBufferBuilder;
use flatstream::{
    read_frame_at, ChecksumDeframer, ChecksumFramer, Crc32, Deframer, StreamReader, StreamWriter,
};
use std::io::{BufReader, Seek, SeekFrom};

const FRAME_COUNT: usize = 1_000;
const PAYLOAD_SIZES: [usize; 2] = [4096, 64 * 1024];

fn finished_builder(size: usize) -> FlatBufferBuilder<'static> {
    let mut builder = FlatBufferBuilder::new();
    let bytes = vec![0xA5u8; size];
    let payload = builder.create_vector(&bytes);
    builder.finish(payload, None);
    builder
}

fn build_file(size: usize) -> (tempfile::NamedTempFile, Vec<u64>) {
    let mut file = tempfile::NamedTempFile::new().expect("tempfile");
    let mut builder = finished_builder(size);
    let mut offsets = Vec::with_capacity(FRAME_COUNT);
    {
        let mut writer = StreamWriter::new(
            std::io::BufWriter::new(file.as_file_mut()),
            ChecksumFramer::new(Crc32::new()),
        );
        for _ in 0..FRAME_COUNT {
            offsets.push(
                writer
                    .write_finished_with_receipt(&mut builder)
                    .unwrap()
                    .frame_start,
            );
        }
        writer.flush().unwrap();
    }
    (file, offsets)
}

fn build_wire(size: usize) -> Vec<u8> {
    let mut wire = Vec::new();
    let mut builder = finished_builder(size);
    let mut writer = StreamWriter::new(&mut wire, ChecksumFramer::new(Crc32::new()));
    for _ in 0..FRAME_COUNT {
        writer.write_finished(&mut builder).unwrap();
    }
    wire
}

fn positioned_reads(c: &mut Criterion) {
    let mut group = c.benchmark_group("Positioned Reads");
    group.throughput(Throughput::Elements(1));

    for size in PAYLOAD_SIZES {
        let (fresh_file, fresh_offsets) = build_file(size);
        let mut fresh_file = fresh_file.into_file();
        let mut fresh_index = 0usize;
        group.bench_function(BenchmarkId::new("fresh_stream_reader", size), |b| {
            b.iter(|| {
                let offset = fresh_offsets[fresh_index % fresh_offsets.len()];
                fresh_index += 1;
                fresh_file.seek(SeekFrom::Start(offset)).unwrap();
                let mut reader = StreamReader::new(
                    BufReader::new(&mut fresh_file),
                    ChecksumDeframer::new(Crc32::new()),
                );
                black_box(reader.read_message().unwrap().unwrap().len())
            });
        });

        let (point_file, point_offsets) = build_file(size);
        let mut point_file = point_file.into_file();
        let mut point_index = 0usize;
        let mut scratch = Vec::new();
        // Warm scratch to the largest frame before measurement.
        read_frame_at(
            &mut point_file,
            &ChecksumDeframer::new(Crc32::new()),
            point_offsets[0],
            &mut scratch,
        )
        .unwrap()
        .unwrap();
        group.bench_function(BenchmarkId::new("read_frame_at", size), |b| {
            b.iter(|| {
                let offset = point_offsets[point_index % point_offsets.len()];
                point_index += 1;
                let frame = read_frame_at(
                    &mut point_file,
                    &ChecksumDeframer::new(Crc32::new()),
                    offset,
                    &mut scratch,
                )
                .unwrap()
                .unwrap();
                black_box(frame.payload.len())
            });
        });

        let (buffered_file, buffered_offsets) = build_file(size);
        let mut buffered_file = BufReader::new(buffered_file.into_file());
        let mut buffered_index = 0usize;
        let mut buffered_scratch = Vec::new();
        read_frame_at(
            &mut buffered_file,
            &ChecksumDeframer::new(Crc32::new()),
            buffered_offsets[0],
            &mut buffered_scratch,
        )
        .unwrap()
        .unwrap();
        group.bench_function(BenchmarkId::new("read_frame_at_buffered", size), |b| {
            b.iter(|| {
                let offset = buffered_offsets[buffered_index % buffered_offsets.len()];
                buffered_index += 1;
                let frame = read_frame_at(
                    &mut buffered_file,
                    &ChecksumDeframer::new(Crc32::new()),
                    offset,
                    &mut buffered_scratch,
                )
                .unwrap()
                .unwrap();
                black_box(frame.payload.len())
            });
        });
    }

    group.finish();
}

fn forward_position_tracking(c: &mut Criterion) {
    let mut group = c.benchmark_group("Forward Position Tracking");
    group.throughput(Throughput::Elements(FRAME_COUNT as u64));

    for size in PAYLOAD_SIZES {
        let wire = build_wire(size);

        let mut raw_source = std::io::Cursor::new(&wire);
        let raw_deframer = ChecksumDeframer::new(Crc32::new());
        let mut raw_scratch = Vec::with_capacity(size + 64);
        group.bench_function(BenchmarkId::new("uncounted_deframer_loop", size), |b| {
            b.iter(|| {
                raw_source.set_position(0);
                let mut frames = 0usize;
                while raw_deframer
                    .read_and_deframe(&mut raw_source, &mut raw_scratch)
                    .unwrap()
                    .is_some()
                {
                    frames += 1;
                }
                black_box(frames)
            });
        });

        let mut counted = StreamReader::with_capacity(
            std::io::Cursor::new(&wire),
            ChecksumDeframer::new(Crc32::new()),
            size + 64,
        );
        group.bench_function(BenchmarkId::new("stream_reader_counted", size), |b| {
            b.iter(|| {
                counted.get_mut().set_position(0);
                let mut frames = 0usize;
                counted
                    .process_all(|payload| {
                        black_box(payload);
                        frames += 1;
                        Ok(())
                    })
                    .unwrap();
                black_box(frames)
            });
        });
    }

    group.finish();
}

criterion_group!(benches, positioned_reads, forward_position_tracking);
criterion_main!(benches);
