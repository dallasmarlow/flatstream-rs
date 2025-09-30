#![cfg(feature = "lobster")]
// Criterion benchmarks for LOBSTER streams (message + orderbook).
//
// What this measures
// -------------------
// - Full-stream read throughput using `StreamReader::process_all`.
// - Primary view: Messages/sec (Melem/s). We avoid pre-counts to prevent cache
//   warming: counts are computed strictly inside the timed loop (iter_custom).
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

fn bench_stream_msgs_only(
    c: &mut Criterion,
    name: &str,
    path: &PathBuf,
    is_message: bool,
    count_sidecar: Option<&PathBuf>,
) {
    let data = fs::read(path).expect("Run `cargo run --example ingest_lobster --release` first");
    let mut group = c.benchmark_group(name);
    group.sample_size(60);
    group.measurement_time(Duration::from_secs(10));

    if let Some(sidecar) = count_sidecar {
        // Use sidecar counts (written at ingestion) to enable native Melem/s reporting
        let text = fs::read_to_string(sidecar).expect("missing counts sidecar");
        let msgs: u64 = text
            .lines()
            .find(|l| l.starts_with("messages:"))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|s| s.trim().parse().ok())
            .expect("invalid messages count");
        group.throughput(Throughput::Elements(msgs));
    }

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

    group.finish();
}

fn benchmark_lobster(c: &mut Criterion) {
    // Build dataset pairs: <stem>-message.bin with matching <stem>-orderbook.bin
    let root = "tests/corpus/lobster";
    let msgs = list_with_suffix(root, "-message.bin").expect("Run ingest example");
    let obs = list_with_suffix(root, "-orderbook.bin").expect("Run ingest example");

    use std::collections::HashMap;
    let mut map: HashMap<String, (Option<PathBuf>, Option<PathBuf>)> = HashMap::new();

    for p in msgs {
        let fname = p.file_name().unwrap().to_string_lossy().to_string();
        let stem = fname
            .strip_suffix("-message.bin")
            .unwrap_or(&fname)
            .to_string();
        map.entry(stem).or_default().0 = Some(p);
    }
    for p in obs {
        let fname = p.file_name().unwrap().to_string_lossy().to_string();
        let stem = fname
            .strip_suffix("-orderbook.bin")
            .unwrap_or(&fname)
            .to_string();
        map.entry(stem).or_default().1 = Some(p);
    }

    let mut had_pair = false;
    for (stem, (m, o)) in map.into_iter() {
        let (mp, op) = match (m, o) {
            (Some(mp), Some(op)) => (mp, op),
            _ => panic!("Incomplete LOBSTER pair for stem: {}", stem),
        };
        had_pair = true;

        let group_name = format!("LOBSTER {}", stem);

        // Per-pair: benchmark message-only, orderbook-only, and combined pair.
        let sidecar = PathBuf::from(format!("{}/{}-counts.txt", root, stem));
        let sidecar_opt = Some(&sidecar);
        bench_stream_msgs_only(
            c,
            &format!("{}/message", group_name),
            &mp,
            true,
            sidecar_opt,
        );
        bench_stream_msgs_only(
            c,
            &format!("{}/orderbook", group_name),
            &op,
            false,
            sidecar_opt,
        );

        // Combined: process both streams in one timed iteration.
        use std::time::Instant;
        let data_m = fs::read(&mp).unwrap();
        let data_o = fs::read(&op).unwrap();
        let mut group = c.benchmark_group(format!("{}/pair", group_name));
        group.sample_size(60);
        group.measurement_time(Duration::from_secs(10));
        // Use sidecar counts to set combined throughput
        let text = fs::read_to_string(&sidecar).expect("missing counts sidecar");
        let msgs: u64 = text
            .lines()
            .find(|l| l.starts_with("messages:"))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|s| s.trim().parse().ok())
            .expect("invalid messages count");
        let obs: u64 = text
            .lines()
            .find(|l| l.starts_with("orderbook:"))
            .and_then(|l| l.split(':').nth(1))
            .and_then(|s| s.trim().parse().ok())
            .expect("invalid orderbook count");

        group.throughput(Throughput::Elements(msgs + obs));
        group.bench_function("read_full_stream_pair", |b| {
            b.iter(|| {
                let mut r1 = StreamReader::new(Cursor::new(&data_m), DefaultDeframer);
                r1.process_all(|payload| {
                    let ev = lobster_generated::message::root_as_message_event(payload).unwrap();
                    black_box(ev);
                    Ok(())
                })
                .unwrap();

                let mut r2 = StreamReader::new(Cursor::new(&data_o), DefaultDeframer);
                r2.process_all(|payload| {
                    let ob =
                        lobster_generated::orderbook::root_as_order_book_snapshot(payload).unwrap();
                    black_box(ob);
                    Ok(())
                })
                .unwrap();
            });
        });
        group.finish();
    }

    if !had_pair {
        panic!("No complete LOBSTER dataset pairs found under {}", root);
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
