use criterion::{black_box, criterion_group, criterion_main, Criterion};
use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::io::Cursor;

struct StrRoot;

impl<'a> StreamDeserialize<'a> for StrRoot {
    type Root = &'a str;
    fn from_payload(payload: &'a [u8]) -> Result<Self::Root> {
        flatbuffers::root::<&'a str>(payload).map_err(Error::FlatbuffersError)
    }
}

fn prepare_buffer(count: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buf), DefaultFramer);
    let mut builder = FlatBufferBuilder::new();
    for i in 0..count {
        builder.reset();
        let s = builder.create_string(&format!("message {i}"));
        builder.finish(s, None);
        writer.write_finished(&mut builder).unwrap();
    }
    buf
}

fn bench_typed_vs_manual(c: &mut Criterion) {
    let mut group = c.benchmark_group("typed_vs_manual/read_only");
    let small = prepare_buffer(1_000);
    let medium = prepare_buffer(10_000);
    let large = prepare_buffer(100_000);

    group.bench_function("manual_small", |b| {
        b.iter(|| {
            let mut reader = StreamReader::new(Cursor::new(&small), DefaultDeframer);
            reader
                .process_all(|payload| {
                    let root = flatbuffers::root::<&str>(payload)?;
                    black_box(root);
                    Ok(())
                })
                .unwrap();
        });
    });

    group.bench_function("manual_medium", |b| {
        b.iter(|| {
            let mut reader = StreamReader::new(Cursor::new(&medium), DefaultDeframer);
            reader
                .process_all(|payload| {
                    let root = flatbuffers::root::<&str>(payload)?;
                    black_box(root);
                    Ok(())
                })
                .unwrap();
        });
    });

    group.bench_function("manual_large", |b| {
        b.iter(|| {
            let mut reader = StreamReader::new(Cursor::new(&large), DefaultDeframer);
            reader
                .process_all(|payload| {
                    let root = flatbuffers::root::<&str>(payload)?;
                    black_box(root);
                    Ok(())
                })
                .unwrap();
        });
    });

    group.bench_function("typed_small", |b| {
        b.iter(|| {
            let mut reader = StreamReader::new(Cursor::new(&small), DefaultDeframer);
            reader
                .process_typed::<StrRoot, _>(|root| {
                    black_box(root);
                    Ok(())
                })
                .unwrap();
        });
    });

    group.bench_function("typed_medium", |b| {
        b.iter(|| {
            let mut reader = StreamReader::new(Cursor::new(&medium), DefaultDeframer);
            reader
                .process_typed::<StrRoot, _>(|root| {
                    black_box(root);
                    Ok(())
                })
                .unwrap();
        });
    });

    group.bench_function("typed_large", |b| {
        b.iter(|| {
            let mut reader = StreamReader::new(Cursor::new(&large), DefaultDeframer);
            reader
                .process_typed::<StrRoot, _>(|root| {
                    black_box(root);
                    Ok(())
                })
                .unwrap();
        });
    });

    #[cfg(feature = "unsafe_typed")]
    {
        group.bench_function("typed_unchecked_small", |b| {
            b.iter(|| {
                let mut reader = StreamReader::new(Cursor::new(&small), DefaultDeframer);
                reader
                    .process_typed_unchecked::<StrRoot, _>(|root| {
                        black_box(root);
                        Ok(())
                    })
                    .unwrap();
            });
        });

        group.bench_function("typed_unchecked_medium", |b| {
            b.iter(|| {
                let mut reader = StreamReader::new(Cursor::new(&medium), DefaultDeframer);
                reader
                    .process_typed_unchecked::<StrRoot, _>(|root| {
                        black_box(root);
                        Ok(())
                    })
                    .unwrap();
            });
        });

        group.bench_function("typed_unchecked_large", |b| {
            b.iter(|| {
                let mut reader = StreamReader::new(Cursor::new(&large), DefaultDeframer);
                reader
                    .process_typed_unchecked::<StrRoot, _>(|root| {
                        black_box(root);
                        Ok(())
                    })
                    .unwrap();
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_typed_vs_manual);
criterion_main!(benches);
