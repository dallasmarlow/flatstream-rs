use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flatbuffers::FlatBufferBuilder;
use flatstream_rs::{DefaultDeframer, DefaultFramer, StreamReader, StreamWriter};
use std::io::Cursor;

fn create_test_message(
    builder: &mut FlatBufferBuilder,
    id: u32,
) -> flatbuffers::WIPOffset<flatbuffers::UnionWIPOffset> {
    let data = builder.create_string(&format!("benchmark message number {}", id));
    builder.finish(data, None);
    flatbuffers::WIPOffset::new(0)
}

fn benchmark_write_with_checksum(c: &mut Criterion) {
    c.bench_function("write_with_checksum", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            for i in 0..100 {
                let message = format!("test message {}", i);
                writer.write(&message).unwrap();
            }

            black_box(buffer);
        });
    });
}

fn benchmark_write_without_checksum(c: &mut Criterion) {
    c.bench_function("write_without_checksum", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            for i in 0..100 {
                let message = format!("test message {}", i);
                writer.write(&message).unwrap();
            }

            black_box(buffer);
        });
    });
}

fn benchmark_read_with_checksum(c: &mut Criterion) {
    // Prepare test data
    let mut buffer = Vec::new();
    {
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        for i in 0..100 {
            let message = format!("test message {}", i);
            writer.write(&message).unwrap();
        }
    }

    c.bench_function("read_with_checksum", |b| {
        b.iter(|| {
            let deframer = DefaultDeframer;
            let reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            for result in reader {
                black_box(result.unwrap());
                count += 1;
            }
            black_box(count);
        });
    });
}

fn benchmark_read_without_checksum(c: &mut Criterion) {
    // Prepare test data
    let mut buffer = Vec::new();
    {
        let framer = DefaultFramer;
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        for i in 0..100 {
            let message = format!("test message {}", i);
            writer.write(&message).unwrap();
        }
    }

    c.bench_function("read_without_checksum", |b| {
        b.iter(|| {
            let deframer = DefaultDeframer;
            let reader = StreamReader::new(Cursor::new(&buffer), deframer);
            let mut count = 0;
            for result in reader {
                black_box(result.unwrap());
                count += 1;
            }
            black_box(count);
        });
    });
}

fn benchmark_write_read_cycle(c: &mut Criterion) {
    c.bench_function("write_read_cycle_with_checksum", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();

            // Write
            {
                let framer = DefaultFramer;
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
                for i in 0..50 {
                    let message = format!("test message {}", i);
                    writer.write(&message).unwrap();
                }
            }

            // Read
            {
                let deframer = DefaultDeframer;
                let reader = StreamReader::new(Cursor::new(&buffer), deframer);
                let mut count = 0;
                for result in reader {
                    black_box(result.unwrap());
                    count += 1;
                }
                black_box(count);
            }
        });
    });
}

fn benchmark_write_batch(c: &mut Criterion) {
    // Create a vector of items to be written.
    let messages: Vec<_> = (0..100).map(|i| format!("message {}", i)).collect();

    c.bench_function("write_batch_100_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            // Call the new batch method
            writer.write_batch(&messages).unwrap();

            black_box(buffer);
        });
    });

    c.bench_function("write_iterative_100_messages", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let framer = DefaultFramer;
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);

            // Call the old iterative method
            for message in &messages {
                writer.write(message).unwrap();
            }

            black_box(buffer);
        });
    });
}

criterion_group!(
    benches,
    benchmark_write_with_checksum,
    benchmark_write_without_checksum,
    benchmark_read_with_checksum,
    benchmark_read_without_checksum,
    benchmark_write_read_cycle,
    benchmark_write_batch,
);
criterion_main!(benches);
