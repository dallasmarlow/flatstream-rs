// Criterion benchmarks for LOBSTER streams (message + orderbook).
//
// What this measures
// -------------------
// - Full-stream read throughput using `StreamReader::process_all`.
// - Two throughput views per file:
//   * Bytes/sec (GiB/s): useful when comparing I/O-bound scenarios.
//   * Messages/sec (Melem/s): stable across framing/checksum options.
//
// Why messages/sec?
// -----------------
// Byte throughput changes with framing overhead (e.g., checksums). Reporting
// messages/sec keeps results comparable across configurations and better tracks
// the deframing+deserialization hot path.
//
// Dataset handling
// ----------------
// - Discovers ALL generated files in `tests/corpus/lobster/` and benchmarks
//   each independently. This avoids biasing the numbers to a single symbol/date.
// - Payloads are deserialized using the checked-in FlatBuffers bindings.
// - `Cursor<&[u8]>` is used to isolate parsing from filesystem I/O.
//
// Zero-copy note
// --------------
// `payload` in the closure is a borrowed slice from the reader’s internal
// buffer. No extra copies are performed on the read path.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use flatstream::{DefaultDeframer, StreamReader};
use std::fs;
use std::io::Cursor;
use std::path::PathBuf;
use std::time::Duration;

mod lobster_generated {
    mod lobster_message_generated {
        #![allow(unused_imports)]
        #![allow(dead_code)]
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/generated/lobster_message_generated.rs"
        ));
    }
    mod lobster_orderbook_generated {
        #![allow(unused_imports)]
        #![allow(dead_code)]
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/examples/generated/lobster_orderbook_generated.rs"
        ));
    }
    pub mod message {
        pub use super::lobster_message_generated::flatstream::lobster::*;
    }
    pub mod orderbook {
        pub use super::lobster_orderbook_generated::flatstream::lobster::*;
    }
}

fn bench_stream(c: &mut Criterion, name: &str, path: &PathBuf, is_message: bool) {
    let data = fs::read(path).expect("Run `cargo run --example ingest_lobster --release` first");
    let mut group = c.benchmark_group(name);
    // Tuning for stability across larger inputs: keep sample count reasonable
    // and allow more time to collect measurements.
    group.sample_size(60);
    group.measurement_time(Duration::from_secs(10));
    group.throughput(Throughput::Bytes(data.len() as u64));
    group.bench_function("read_full_stream", |b| {
        b.iter(|| {
            let mut r = StreamReader::new(Cursor::new(&data), DefaultDeframer);
            r.process_all(|payload| {
                if is_message {
                    let ev = lobster_generated::message::root_as_message_event(payload).unwrap();
                    black_box(ev);
                } else {
                    let ob =
                        lobster_generated::orderbook::root_as_order_book_snapshot(payload).unwrap();
                    black_box(ob);
                }
                Ok(())
            })
            .unwrap();
        });
    });

    // Also report messages/sec by counting frames once and setting element throughput
    let mut msg_count = 0u64;
    {
        let mut r = StreamReader::new(Cursor::new(&data), DefaultDeframer);
        r.process_all(|payload| {
            if is_message {
                let ev = lobster_generated::message::root_as_message_event(payload).unwrap();
                black_box(ev);
            } else {
                let ob =
                    lobster_generated::orderbook::root_as_order_book_snapshot(payload).unwrap();
                black_box(ob);
            }
            msg_count += 1;
            Ok(())
        })
        .unwrap();
    }
    group.throughput(Throughput::Elements(msg_count));
    group.bench_function("read_full_stream_msgs", |b| {
        b.iter(|| {
            let mut r = StreamReader::new(Cursor::new(&data), DefaultDeframer);
            let mut count = 0u64;
            r.process_all(|payload| {
                if is_message {
                    let ev = lobster_generated::message::root_as_message_event(payload).unwrap();
                    black_box(ev);
                } else {
                    let ob =
                        lobster_generated::orderbook::root_as_order_book_snapshot(payload).unwrap();
                    black_box(ob);
                }
                count += 1;
                Ok(())
            })
            .unwrap();
            black_box(count);
        });
    });
    group.finish();
}

fn benchmark_lobster(c: &mut Criterion) {
    let msgs =
        list_with_suffix("tests/corpus/lobster", "-message.bin").expect("Run ingest example");
    let obs =
        list_with_suffix("tests/corpus/lobster", "-orderbook.bin").expect("Run ingest example");
    for p in msgs {
        let name = format!(
            "LOBSTER Message {}",
            p.file_name().unwrap().to_string_lossy()
        );
        bench_stream(c, &name, &p, true);
    }
    for p in obs {
        let name = format!(
            "LOBSTER Orderbook {}",
            p.file_name().unwrap().to_string_lossy()
        );
        bench_stream(c, &name, &p, false);
    }
}

criterion_group!(benches, benchmark_lobster);
criterion_main!(benches);

fn list_with_suffix(dir: &str, suffix: &str) -> Option<Vec<PathBuf>> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    Some(
        entries
            .into_iter()
            .filter(|p| p.is_file() && p.to_string_lossy().ends_with(suffix))
            .collect(),
    )
}
