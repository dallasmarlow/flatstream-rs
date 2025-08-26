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
    // ---
    // # Benchmark Purpose: Typed vs. Manual Root Access
    //
    // Central question: Does the typed reading API add overhead relative to manual
    // `flatbuffers::root` access? What about the unchecked, unsafe variant?
    //
    // Design: Prebuild buffers with 1k/10k/100k string messages, then compare:
    // - manual_*: manual `flatbuffers::root::<&str>` within `process_all`
    // - typed_*: `process_typed::<StrRoot, _>` using `StreamDeserialize`
    // - typed_unchecked_*: `process_typed_unchecked` (feature-gated), skipping verification
    //
    // Takeaway:
    // - typed_* should match manual_* throughput for valid data, with better ergonomics/safety
    // - typed_unchecked_* can be faster but is only safe for trusted data
    // ---
    let mut group = c.benchmark_group("typed_vs_manual/read_only");
    let small = prepare_buffer(1_000);
    let medium = prepare_buffer(10_000);
    let large = prepare_buffer(100_000);

    // Manual verification path: baseline correctness and performance
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

    // Typed, safe path: should be equivalent to manual for valid inputs
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
        // Unchecked typed path: skips verification; use only for trusted data
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
