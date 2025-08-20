// benches/simple_benchmarks.rs
// Simple, high-throughput micro-benchmarks on primitive-type payloads.
// Compares flatstream (default and unsafe read) with bincode and serde_json.

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flatbuffers::FlatBufferBuilder;
use flatstream::{
    BoundedDeframer, BoundedFramer, DefaultDeframer, DefaultFramer, StreamReader, StreamSerialize,
    StreamWriter, UnsafeDeframer,
};
use std::io::{Cursor, Read, Write};

#[cfg(feature = "comparative_bench")]
use serde::{Deserialize, Serialize};

// Data shapes

#[cfg_attr(feature = "comparative_bench", derive(Serialize, Deserialize))]
#[derive(Clone, Debug)]
struct MinimalNumeric {
    a: u64,
    b: u64,
    c: u64,
}

impl StreamSerialize for MinimalNumeric {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> flatstream::Result<()> {
        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&self.a.to_le_bytes());
        data.extend_from_slice(&self.b.to_le_bytes());
        data.extend_from_slice(&self.c.to_le_bytes());
        let vec = builder.create_vector(&data);
        builder.finish(vec, None);
        Ok(())
    }
}

#[cfg_attr(feature = "comparative_bench", derive(Serialize, Deserialize))]
#[derive(Clone, Debug)]
struct MinimalString {
    s: String,
}

impl StreamSerialize for MinimalString {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> flatstream::Result<()> {
        let s = builder.create_string(&self.s);
        builder.finish(s, None);
        Ok(())
    }
}

const COUNT: usize = 100;

fn make_minimal_numeric(count: usize) -> Vec<MinimalNumeric> {
    (0..count as u64)
        .map(|i| MinimalNumeric {
            a: i,
            b: i.wrapping_mul(3),
            c: i.wrapping_add(7),
        })
        .collect()
}

fn make_minimal_string(count: usize) -> Vec<MinimalString> {
    let s = "0123456789abcdef".to_string();
    (0..count).map(|_| MinimalString { s: s.clone() }).collect()
}

// Simple Streams (Primitive Types): write+read cycles

fn bench_simple_numeric_write_read_cycle(c: &mut Criterion) {
    let mut group = c.benchmark_group("Simple Streams (Numeric)/write_read_cycle_100");
    let events = make_minimal_numeric(COUNT);

    group.bench_function("flatstream_default", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            {
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
                let mut builder = FlatBufferBuilder::new();
                for e in &events {
                    builder.reset();
                    flatstream::StreamSerialize::serialize(e, &mut builder).unwrap();
                    writer.write_finished(&mut builder).unwrap();
                }
            }
            let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box((buffer, count));
        });
    });

    // Bounded adapters on both write and read paths (under very large limit)
    group.bench_function("flatstream_bounded", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            {
                let mut writer = StreamWriter::new(
                    Cursor::new(&mut buffer),
                    BoundedFramer::new(DefaultFramer, 1 << 30),
                );
                let mut builder = FlatBufferBuilder::new();
                for e in &events {
                    builder.reset();
                    flatstream::StreamSerialize::serialize(e, &mut builder).unwrap();
                    writer.write_finished(&mut builder).unwrap();
                }
            }
            let mut reader = StreamReader::new(
                Cursor::new(&buffer),
                BoundedDeframer::new(DefaultDeframer, 1 << 30),
            );
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box((buffer, count));
        });
    });

    group.bench_function("flatstream_default_unsafe_read", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            {
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
                let mut builder = FlatBufferBuilder::new();
                for e in &events {
                    builder.reset();
                    flatstream::StreamSerialize::serialize(e, &mut builder).unwrap();
                    writer.write_finished(&mut builder).unwrap();
                }
            }
            let mut reader = StreamReader::new(Cursor::new(&buffer), UnsafeDeframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box((buffer, count));
        });
    });

    group.bench_function("bincode", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            for e in &events {
                let encoded = bincode::serialize(e).unwrap();
                let len = encoded.len() as u32;
                buffer.write_all(&len.to_le_bytes()).unwrap();
                buffer.write_all(&encoded).unwrap();
            }
            let mut reader = Cursor::new(&buffer);
            let mut len_bytes = [0u8; 4];
            let mut count = 0;
            while reader.read_exact(&mut len_bytes).is_ok() {
                let len = u32::from_le_bytes(len_bytes) as usize;
                let mut msg = vec![0u8; len];
                reader.read_exact(&mut msg).unwrap();
                let _: MinimalNumeric = bincode::deserialize(&msg).unwrap();
                count += 1;
            }
            black_box((buffer, count));
        });
    });

    group.bench_function("serde_json", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            for e in &events {
                let encoded = serde_json::to_vec(e).unwrap();
                let len = encoded.len() as u32;
                buffer.write_all(&len.to_le_bytes()).unwrap();
                buffer.write_all(&encoded).unwrap();
            }
            let mut reader = Cursor::new(&buffer);
            let mut len_bytes = [0u8; 4];
            let mut count = 0;
            while reader.read_exact(&mut len_bytes).is_ok() {
                let len = u32::from_le_bytes(len_bytes) as usize;
                let mut msg = vec![0u8; len];
                reader.read_exact(&mut msg).unwrap();
                let _: MinimalNumeric = serde_json::from_slice(&msg).unwrap();
                count += 1;
            }
            black_box((buffer, count));
        });
    });

    group.finish();
}

fn bench_simple_string_write_read_cycle(c: &mut Criterion) {
    let mut group = c.benchmark_group("Simple Streams (String16)/write_read_cycle_100");
    let events = make_minimal_string(COUNT);

    group.bench_function("flatstream_default", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            {
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
                let mut builder = FlatBufferBuilder::new();
                for e in &events {
                    builder.reset();
                    flatstream::StreamSerialize::serialize(e, &mut builder).unwrap();
                    writer.write_finished(&mut builder).unwrap();
                }
            }
            let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box((buffer, count));
        });
    });

    // Bounded adapters on both write and read paths (under very large limit)
    group.bench_function("flatstream_bounded", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            {
                let mut writer = StreamWriter::new(
                    Cursor::new(&mut buffer),
                    BoundedFramer::new(DefaultFramer, 1 << 30),
                );
                let mut builder = FlatBufferBuilder::new();
                for e in &events {
                    builder.reset();
                    flatstream::StreamSerialize::serialize(e, &mut builder).unwrap();
                    writer.write_finished(&mut builder).unwrap();
                }
            }
            let mut reader = StreamReader::new(
                Cursor::new(&buffer),
                BoundedDeframer::new(DefaultDeframer, 1 << 30),
            );
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box((buffer, count));
        });
    });

    group.bench_function("flatstream_default_unsafe_read", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            {
                let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
                let mut builder = FlatBufferBuilder::new();
                for e in &events {
                    builder.reset();
                    flatstream::StreamSerialize::serialize(e, &mut builder).unwrap();
                    writer.write_finished(&mut builder).unwrap();
                }
            }
            let mut reader = StreamReader::new(Cursor::new(&buffer), UnsafeDeframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box((buffer, count));
        });
    });

    group.bench_function("bincode", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            for e in &events {
                let encoded = bincode::serialize(e).unwrap();
                let len = encoded.len() as u32;
                buffer.write_all(&len.to_le_bytes()).unwrap();
                buffer.write_all(&encoded).unwrap();
            }
            let mut reader = Cursor::new(&buffer);
            let mut len_bytes = [0u8; 4];
            let mut count = 0;
            while reader.read_exact(&mut len_bytes).is_ok() {
                let len = u32::from_le_bytes(len_bytes) as usize;
                let mut msg = vec![0u8; len];
                reader.read_exact(&mut msg).unwrap();
                let _: MinimalString = bincode::deserialize(&msg).unwrap();
                count += 1;
            }
            black_box((buffer, count));
        });
    });

    group.bench_function("serde_json", |b| {
        b.iter(|| {
            let mut buffer = Vec::new();
            for e in &events {
                let encoded = serde_json::to_vec(e).unwrap();
                let len = encoded.len() as u32;
                buffer.write_all(&len.to_le_bytes()).unwrap();
                buffer.write_all(&encoded).unwrap();
            }
            let mut reader = Cursor::new(&buffer);
            let mut len_bytes = [0u8; 4];
            let mut count = 0;
            while reader.read_exact(&mut len_bytes).is_ok() {
                let len = u32::from_le_bytes(len_bytes) as usize;
                let mut msg = vec![0u8; len];
                reader.read_exact(&mut msg).unwrap();
                let _: MinimalString = serde_json::from_slice(&msg).unwrap();
                count += 1;
            }
            black_box((buffer, count));
        });
    });

    group.finish();
}

// Read-only deframer isolation

fn bench_simple_numeric_read_only(c: &mut Criterion) {
    let mut group = c.benchmark_group("Simple Streams (Numeric)/read_only_100");

    let mut buffer = Vec::new();
    {
        let events = make_minimal_numeric(COUNT);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();
        for e in &events {
            builder.reset();
            flatstream::StreamSerialize::serialize(e, &mut builder).unwrap();
            writer.write_finished(&mut builder).unwrap();
        }
    }

    group.bench_function("default_deframer", |b| {
        b.iter(|| {
            let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box(count);
        });
    });

    group.bench_function("bounded_deframer", |b| {
        b.iter(|| {
            let mut reader = StreamReader::new(
                Cursor::new(&buffer),
                BoundedDeframer::new(DefaultDeframer, 1 << 30),
            );
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box(count);
        });
    });

    group.bench_function("unsafe_deframer", |b| {
        b.iter(|| {
            let mut reader = StreamReader::new(Cursor::new(&buffer), UnsafeDeframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box(count);
        });
    });

    group.finish();
}

fn bench_simple_string_read_only(c: &mut Criterion) {
    let mut group = c.benchmark_group("Simple Streams (String16)/read_only_100");

    let mut buffer = Vec::new();
    {
        let events = make_minimal_string(COUNT);
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();
        for e in &events {
            builder.reset();
            flatstream::StreamSerialize::serialize(e, &mut builder).unwrap();
            writer.write_finished(&mut builder).unwrap();
        }
    }

    group.bench_function("default_deframer", |b| {
        b.iter(|| {
            let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box(count);
        });
    });

    group.bench_function("bounded_deframer", |b| {
        b.iter(|| {
            let mut reader = StreamReader::new(
                Cursor::new(&buffer),
                BoundedDeframer::new(DefaultDeframer, 1 << 30),
            );
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box(count);
        });
    });

    group.bench_function("unsafe_deframer", |b| {
        b.iter(|| {
            let mut reader = StreamReader::new(Cursor::new(&buffer), UnsafeDeframer);
            let mut count = 0;
            reader
                .process_all(|_payload| {
                    count += 1;
                    Ok(())
                })
                .unwrap();
            black_box(count);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_simple_numeric_write_read_cycle,
    bench_simple_string_write_read_cycle,
    bench_simple_numeric_read_only,
    bench_simple_string_read_only
);
criterion_main!(benches);
