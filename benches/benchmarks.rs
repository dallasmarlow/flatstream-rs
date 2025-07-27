use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flatbuffers::FlatBufferBuilder;
use flatstream_rs::{ChecksumType, StreamReader, StreamWriter};
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
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), ChecksumType::XxHash64);

            for i in 0..100 {
                let mut builder = FlatBufferBuilder::new();
                create_test_message(&mut builder, i);
                writer.write_message(&mut builder).unwrap();
            }

            black_box(buffer);
        });
    });
}

fn benchmark_write_without_checksum(c: &mut Criterion) {
    c.bench_function("write_without_checksum", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            let mut writer = StreamWriter::new(Cursor::new(&mut buffer), ChecksumType::None);

            for i in 0..100 {
                let mut builder = FlatBufferBuilder::new();
                create_test_message(&mut builder, i);
                writer.write_message(&mut builder).unwrap();
            }

            black_box(buffer);
        });
    });
}

fn benchmark_read_with_checksum(c: &mut Criterion) {
    // Prepare test data
    let mut buffer = Vec::new();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), ChecksumType::XxHash64);
        for i in 0..100 {
            let mut builder = FlatBufferBuilder::new();
            create_test_message(&mut builder, i);
            writer.write_message(&mut builder).unwrap();
        }
    }

    c.bench_function("read_with_checksum", |b| {
        b.iter(|| {
            let reader = StreamReader::new(Cursor::new(&buffer), ChecksumType::XxHash64);
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
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), ChecksumType::None);
        for i in 0..100 {
            let mut builder = FlatBufferBuilder::new();
            create_test_message(&mut builder, i);
            writer.write_message(&mut builder).unwrap();
        }
    }

    c.bench_function("read_without_checksum", |b| {
        b.iter(|| {
            let reader = StreamReader::new(Cursor::new(&buffer), ChecksumType::None);
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
                let mut writer =
                    StreamWriter::new(Cursor::new(&mut buffer), ChecksumType::XxHash64);
                for i in 0..50 {
                    let mut builder = FlatBufferBuilder::new();
                    create_test_message(&mut builder, i);
                    writer.write_message(&mut builder).unwrap();
                }
            }

            // Read
            {
                let reader = StreamReader::new(Cursor::new(&buffer), ChecksumType::XxHash64);
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

criterion_group!(
    benches,
    benchmark_write_with_checksum,
    benchmark_write_without_checksum,
    benchmark_read_with_checksum,
    benchmark_read_without_checksum,
    benchmark_write_read_cycle,
);
criterion_main!(benches);
