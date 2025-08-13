use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use flatbuffers::FlatBufferBuilder;
use flatstream::framing::{BoundedDeframer, BoundedFramer, DeframerExt, FramerExt, ObserverDeframer, ObserverFramer};
use flatstream::{DefaultDeframer, DefaultFramer, Result, StreamReader};
use flatstream::Framer;
use std::cell::Cell;
use std::io::Cursor;
use std::io::{BufReader, BufWriter, Read, Write};
use std::time::Duration;
use tempfile::NamedTempFile;

fn build_payload(len: usize) -> Vec<u8> {
    let mut b = FlatBufferBuilder::new();
    let s = b.create_string(&"x".repeat(len));
    b.finish(s, None);
    b.finished_data().to_vec()
}

fn write_with_framer(payload: &[u8], framer: &impl flatstream::framing::Framer) -> Result<usize> {
    let mut out = Vec::with_capacity(4 + payload.len());
    // Expert-mode write_finished path
    framer.frame_and_write(&mut out, payload)?;
    Ok(out.len())
}

fn build_framed_bytes(payload: &[u8], framer: &impl flatstream::framing::Framer) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    framer.frame_and_write(&mut out, payload).unwrap();
    out
}

fn bench_bounded_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("bounded_write");
    let payload = build_payload(32);

    group.throughput(Throughput::Bytes(payload.len() as u64));

    group.bench_function("baseline_default", |b| {
        b.iter(|| write_with_framer(black_box(&payload), &DefaultFramer).unwrap())
    });

    group.bench_function("bounded_under_limit_explicit", |b| {
        let framer = BoundedFramer::new(DefaultFramer, 1 << 20);
        b.iter(|| write_with_framer(black_box(&payload), &framer).unwrap())
    });

    group.bench_function("bounded_under_limit_fluent", |b| {
        let framer = DefaultFramer.bounded(1 << 20);
        b.iter(|| write_with_framer(black_box(&payload), &framer).unwrap())
    });

    // Over-limit fast-fail path
    group.bench_function("bounded_over_limit_error", |b| {
        b.iter(|| {
            let framer = BoundedFramer::new(DefaultFramer, 4);
            let _ = framer.frame_and_write(&mut std::io::sink(), black_box(&payload));
        })
    });

    group.finish();
}

fn bench_bounded_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("bounded_read");
    let payload = build_payload(64);
    let bytes = build_framed_bytes(&payload, &DefaultFramer);

    group.throughput(Throughput::Bytes(payload.len() as u64));

    group.bench_function("baseline_default", |b| {
        b.iter(|| {
            let mut reader = StreamReader::new(Cursor::new(&bytes), DefaultDeframer);
            reader.process_all(|p| {
                black_box(p);
                Ok(())
            })
            .unwrap()
        })
    });

    group.bench_function("bounded_under_limit_explicit", |b| {
        b.iter(|| {
            let deframer = BoundedDeframer::new(DefaultDeframer, 1 << 20);
            let mut reader = StreamReader::new(Cursor::new(&bytes), deframer);
            reader.process_all(|p| {
                black_box(p);
                Ok(())
            })
            .unwrap()
        })
    });

    group.bench_function("bounded_under_limit_fluent", |b| {
        b.iter(|| {
            let deframer = DefaultDeframer.bounded(1 << 20);
            let mut reader = StreamReader::new(Cursor::new(&bytes), deframer);
            reader.process_all(|p| {
                black_box(p);
                Ok(())
            })
            .unwrap()
        })
    });

    // Over-limit fast-stop path: frame with length 64 but limit 16
    let over_bytes = bytes.clone();
    group.bench_function("bounded_over_limit_error", |b| {
        b.iter(|| {
            let deframer = BoundedDeframer::new(DefaultDeframer, 16);
            let mut reader = StreamReader::new(Cursor::new(&over_bytes), deframer);
            let _ = reader.process_all(|_| Ok(()));
        })
    });

    group.finish();
}

fn bench_observer_write_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("observer_adapters");
    let payload = build_payload(48);

    group.throughput(Throughput::Bytes(payload.len() as u64));

    group.bench_function("write_baseline", |b| {
        b.iter(|| write_with_framer(black_box(&payload), &DefaultFramer).unwrap())
    });

    group.bench_function("write_observer", |b| {
        let counter = Cell::new(0usize);
        b.iter(|| {
            let framer = ObserverFramer::new(DefaultFramer, |p: &[u8]| counter.set(counter.get() + p.len()));
            write_with_framer(black_box(&payload), &framer).unwrap()
        })
    });

    let bytes = build_framed_bytes(&payload, &DefaultFramer);

    group.bench_function("read_baseline", |b| {
        b.iter(|| {
            let mut reader = StreamReader::new(Cursor::new(&bytes), DefaultDeframer);
            reader.process_all(|p| {
                black_box(p);
                Ok(())
            })
            .unwrap()
        })
    });

    group.bench_function("read_observer", |b| {
        let msgs = Cell::new(0usize);
        b.iter(|| {
            let deframer = ObserverDeframer::new(DefaultDeframer, |_p: &[u8]| msgs.set(msgs.get() + 1));
            let mut reader = StreamReader::new(Cursor::new(&bytes), deframer);
            reader.process_all(|p| {
                black_box(p);
                Ok(())
            })
            .unwrap()
        })
    });

    group.finish();
}

fn bench_reader_capacity(c: &mut Criterion) {
    let mut group = c.benchmark_group("reader_capacity");
    let payload = build_payload(256);
    let bytes = build_framed_bytes(&payload, &DefaultFramer);

    for &cap in &[0usize, 1024usize, 4096usize] {
        group.bench_with_input(BenchmarkId::from_parameter(cap), &cap, |b, &cap| {
            b.iter(|| {
                let mut reader = StreamReader::with_capacity(Cursor::new(&bytes), DefaultDeframer, cap);
                reader.process_all(|p| {
                    black_box(p);
                    Ok(())
                })
                .unwrap()
            })
        });
    }

    group.finish();
}

fn bench_bounded_write_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("bounded_write_sizes");
    for &sz in &[64usize, 1024, 64 * 1024] {
        let payload = build_payload(sz);
        group.throughput(Throughput::Bytes(sz as u64));

        group.bench_with_input(BenchmarkId::new("baseline_default", sz), &payload, |b, p| {
            b.iter(|| write_with_framer(black_box(p), &DefaultFramer).unwrap())
        });

        group.bench_with_input(BenchmarkId::new("bounded_under_limit", sz), &payload, |b, p| {
            let framer = DefaultFramer.bounded(1 << 30);
            b.iter(|| write_with_framer(black_box(p), &framer).unwrap())
        });
    }
    group.finish();
}

fn bench_bounded_read_sizes(c: &mut Criterion) {
    let mut group = c.benchmark_group("bounded_read_sizes");
    for &sz in &[64usize, 1024, 64 * 1024] {
        let payload = build_payload(sz);
        let bytes = build_framed_bytes(&payload, &DefaultFramer);
        group.throughput(Throughput::Bytes(sz as u64));

        group.bench_with_input(BenchmarkId::new("baseline_default", sz), &bytes, |b, data| {
            b.iter(|| {
                let mut reader = StreamReader::new(Cursor::new(data), DefaultDeframer);
                reader.process_all(|p| {
                    black_box(p);
                    Ok(())
                })
                .unwrap()
            })
        });

        group.bench_with_input(BenchmarkId::new("bounded_under_limit", sz), &bytes, |b, data| {
            b.iter(|| {
                let deframer = DefaultDeframer.bounded(1 << 30);
                let mut reader = StreamReader::new(Cursor::new(data), deframer);
                reader.process_all(|p| {
                    black_box(p);
                    Ok(())
                })
                .unwrap()
            })
        });
    }
    group.finish();
}

fn bench_file_io_adapters(c: &mut Criterion) {
    let mut group = c.benchmark_group("file_io_adapters");
    group.measurement_time(Duration::from_secs(10));
    let payload = build_payload(8 * 1024); // 8KB typical

    group.bench_function("file_write_baseline", |b| {
        b.iter(|| {
            let tmp = NamedTempFile::new().unwrap();
            let mut writer = BufWriter::new(tmp.reopen().unwrap());
            let len = write_with_framer(&payload, &DefaultFramer).unwrap();
            let framed = build_framed_bytes(&payload, &DefaultFramer);
            writer.write_all(&framed).unwrap();
            writer.flush().unwrap();
            black_box(len)
        })
    });

    group.bench_function("file_write_bounded", |b| {
        let framer = DefaultFramer.bounded(1 << 30);
        b.iter(|| {
            let tmp = NamedTempFile::new().unwrap();
            let mut writer = BufWriter::new(tmp.reopen().unwrap());
            let framed = build_framed_bytes(&payload, &framer);
            writer.write_all(&framed).unwrap();
            writer.flush().unwrap();
            black_box(())
        })
    });

    let data = build_framed_bytes(&payload, &DefaultFramer);

    group.bench_function("file_read_baseline", |b| {
        b.iter(|| {
            let tmp = NamedTempFile::new().unwrap();
            std::fs::write(tmp.path(), &data).unwrap();
            let file = std::fs::File::open(tmp.path()).unwrap();
            let mut reader = StreamReader::new(BufReader::new(file), DefaultDeframer);
            reader.process_all(|p| {
                black_box(p);
                Ok(())
            })
            .unwrap();
        })
    });

    group.bench_function("file_read_bounded", |b| {
        b.iter(|| {
            let tmp = NamedTempFile::new().unwrap();
            std::fs::write(tmp.path(), &data).unwrap();
            let file = std::fs::File::open(tmp.path()).unwrap();
            let deframer = DefaultDeframer.bounded(1 << 30);
            let mut reader = StreamReader::new(BufReader::new(file), deframer);
            reader.process_all(|p| {
                black_box(p);
                Ok(())
            })
            .unwrap();
        })
    });

    group.finish();
}

fn adapters_micro(c: &mut Criterion) {
    bench_bounded_write(c);
    bench_bounded_read(c);
    bench_observer_write_read(c);
    bench_reader_capacity(c);
    bench_bounded_write_sizes(c);
    bench_bounded_read_sizes(c);
    bench_file_io_adapters(c);
}

criterion_group!(name = micro; config = Criterion::default(); targets = adapters_micro);
criterion_main!(micro);


