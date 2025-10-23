use csv::{ReaderBuilder, StringRecord};
use flatbuffers::FlatBufferBuilder;
use flatstream::{DefaultFramer, StreamWriter};
use std::fs::{self, File};
use std::io::{BufWriter, Read};
use std::path::Path;
use thiserror::Error;
use zip::read::ZipArchive;
#[cfg(feature = "lobster")]
#[path = "../tests/harness/lobster_common.rs"]
mod lobster_common;

// Include checked-in generated code so no build-time codegen is required.
// Generate once with:
// flatc --rust -o examples/generated examples/schemas/lobster_message.fbs examples/schemas/lobster_orderbook.fbs
#[allow(dead_code)]
mod lobster_generated {
    pub mod message {
        #![allow(unused_imports)]
        #![allow(dead_code)]
        #![allow(mismatched_lifetime_syntaxes)]
        #![allow(clippy::extra_unused_lifetimes)]
        #![allow(clippy::derivable_impls)]
        include!("generated/lobster_message_generated.rs");
        pub use self::flatstream::lobster::*;
    }
    pub mod orderbook {
        #![allow(unused_imports)]
        #![allow(dead_code)]
        #![allow(mismatched_lifetime_syntaxes)]
        #![allow(clippy::extra_unused_lifetimes)]
        #![allow(clippy::derivable_impls)]
        include!("generated/lobster_orderbook_generated.rs");
        pub use self::flatstream::lobster::*;
    }
}

#[derive(Debug, Error)]
enum IngestError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("csv: {0}")]
    Csv(#[from] csv::Error),
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("parse int: {0}")]
    ParseInt(#[from] std::num::ParseIntError),
    #[error("parse float: {0}")]
    ParseFloat(#[from] std::num::ParseFloatError),
    #[error("missing field: {0}")]
    Missing(&'static str),
    #[error("no matching CSV entry in {0}")]
    MissingCsv(String),
    #[error("length mismatch: messages={messages} orderbook={orderbook}")]
    LengthMismatch { messages: usize, orderbook: usize },
    #[error("zip contains only one CSV type (message/orderbook) for {zip}")]
    MissingPair { zip: String },
    #[error("flatstream: {0}")]
    Flatstream(#[from] flatstream::error::Error),
}

type IngestResult<T> = std::result::Result<T, IngestError>;

struct MessageRow {
    timestamp: f64,
    event_type: u8,
    order_id: u64,
    size: u32,
    price: i64,
    direction: i8,
}

fn parse_message_row(rec: &StringRecord) -> IngestResult<MessageRow> {
    fn get<'a>(rec: &'a StringRecord, idx: usize, name: &'static str) -> IngestResult<&'a str> {
        rec.get(idx).ok_or(IngestError::Missing(name))
    }
    let timestamp: f64 = get(rec, 0, "time")?.parse()?;
    let event_type: u8 = get(rec, 1, "type")?.parse()?;
    let order_id: u64 = get(rec, 2, "order_id")?.parse()?;
    let size: u32 = get(rec, 3, "size")?.parse()?;
    let price_scaled: i64 = get(rec, 4, "price")?.parse()?; // already x10000
    let direction: i8 = get(rec, 5, "direction")?.parse()?;

    Ok(MessageRow {
        timestamp,
        event_type,
        order_id,
        size,
        price: price_scaled,
        direction,
    })
}

// Keep for potential future single-file ingestion; silences unused warnings.
#[allow(dead_code)]
fn write_message_stream<R: Read>(
    mut rdr: csv::Reader<R>,
    output_path: &Path,
) -> IngestResult<Vec<f64>> {
    use lobster_generated::message as lobster;

    let out = BufWriter::new(File::create(output_path)?);
    let mut writer = StreamWriter::new(out, DefaultFramer);
    let mut builder = FlatBufferBuilder::new();
    let mut timestamps: Vec<f64> = Vec::new();

    for result in rdr.records() {
        let rec = result?;
        let row = parse_message_row(&rec)?;
        timestamps.push(row.timestamp);

        builder.reset();
        let args = lobster::MessageEventArgs {
            timestamp: row.timestamp,
            event_type: row.event_type,
            order_id: row.order_id,
            size: row.size,
            price: row.price,
            direction: row.direction,
        };
        let off = lobster::MessageEvent::create(&mut builder, &args);
        builder.finish(off, None);
        writer.write_finished(&mut builder)?;
    }

    writer.flush()?;
    Ok(timestamps)
}

// Keep for potential future single-file ingestion; silences unused warnings.
#[allow(dead_code)]
fn write_orderbook_stream<R: Read>(
    mut rdr: csv::Reader<R>,
    output_path: &Path,
    message_timestamps: Option<&[f64]>,
) -> IngestResult<()> {
    use lobster_generated::orderbook as lobster;

    let out = BufWriter::new(File::create(output_path)?);
    let mut writer = StreamWriter::new(out, DefaultFramer);
    let mut builder = FlatBufferBuilder::new();

    for (row_idx, result) in rdr.records().enumerate() {
        let rec = result?;
        // Orderbook rows are Nx(4*L). Columns: [AskP1, AskS1, BidP1, BidS1, AskP2, AskS2, BidP2, BidS2, ...]
        // Map to vectors of Level(price,size) for asks and bids per schema.
        let mut asks: Vec<lobster::Level> = Vec::new();
        let mut bids: Vec<lobster::Level> = Vec::new();

        let mut i = 0usize;
        while i + 3 < rec.len() {
            let ap: i64 = rec.get(i).unwrap().parse()?;
            let asz: u64 = rec.get(i + 1).unwrap().parse()?;
            let bp: i64 = rec.get(i + 2).unwrap().parse()?;
            let bsz: u64 = rec.get(i + 3).unwrap().parse()?;
            asks.push(lobster::Level::new(ap, asz));
            bids.push(lobster::Level::new(bp, bsz));
            i += 4;
        }

        builder.reset();
        let asks_fb = builder.create_vector(&asks);
        let bids_fb = builder.create_vector(&bids);
        let ts = message_timestamps
            .and_then(|v| v.get(row_idx).copied())
            .unwrap_or(0.0);
        let args = lobster::OrderBookSnapshotArgs {
            timestamp: ts,
            asks: Some(asks_fb),
            bids: Some(bids_fb),
        };
        let off = lobster::OrderBookSnapshot::create(&mut builder, &args);
        builder.finish(off, None);
        writer.write_finished(&mut builder)?;
    }

    writer.flush()?;
    Ok(())
}

fn write_pair_streams<R1: Read, R2: Read>(
    mut msg_rdr: csv::Reader<R1>,
    mut ob_rdr: csv::Reader<R2>,
    msg_out: &Path,
    ob_out: &Path,
) -> IngestResult<(usize, usize)> {
    use lobster_generated::{message as lm, orderbook as lo};

    let mut msg_writer = StreamWriter::new(BufWriter::new(File::create(msg_out)?), DefaultFramer);
    let mut ob_writer = StreamWriter::new(BufWriter::new(File::create(ob_out)?), DefaultFramer);

    let mut msg_builder = FlatBufferBuilder::new();
    let mut ob_builder = FlatBufferBuilder::new();

    let mut msg_iter = msg_rdr.records();
    let mut ob_iter = ob_rdr.records();
    let mut msg_count = 0usize;
    let mut ob_count = 0usize;

    loop {
        match (msg_iter.next(), ob_iter.next()) {
            (None, None) => break,
            (Some(Ok(mrec)), Some(Ok(orec))) => {
                // Message
                let mr = parse_message_row(&mrec)?;
                msg_builder.reset();
                let margs = lm::MessageEventArgs {
                    timestamp: mr.timestamp,
                    event_type: mr.event_type,
                    order_id: mr.order_id,
                    size: mr.size,
                    price: mr.price,
                    direction: mr.direction,
                };
                let moff = lm::MessageEvent::create(&mut msg_builder, &margs);
                msg_builder.finish(moff, None);
                msg_writer.write_finished(&mut msg_builder)?;
                msg_count += 1;

                // Orderbook row -> asks/bids
                let cap = orec.len() / 4;
                let mut asks: Vec<lo::Level> = Vec::with_capacity(cap);
                let mut bids: Vec<lo::Level> = Vec::with_capacity(cap);
                let mut i = 0usize;
                while i + 3 < orec.len() {
                    let ap: i64 = orec.get(i).unwrap().parse()?;
                    let asz: u64 = orec.get(i + 1).unwrap().parse()?;
                    let bp: i64 = orec.get(i + 2).unwrap().parse()?;
                    let bsz: u64 = orec.get(i + 3).unwrap().parse()?;
                    asks.push(lo::Level::new(ap, asz));
                    bids.push(lo::Level::new(bp, bsz));
                    i += 4;
                }
                ob_builder.reset();
                let asks_fb = ob_builder.create_vector(&asks);
                let bids_fb = ob_builder.create_vector(&bids);
                let oargs = lo::OrderBookSnapshotArgs {
                    timestamp: mr.timestamp,
                    asks: Some(asks_fb),
                    bids: Some(bids_fb),
                };
                let ooff = lo::OrderBookSnapshot::create(&mut ob_builder, &oargs);
                ob_builder.finish(ooff, None);
                ob_writer.write_finished(&mut ob_builder)?;
                ob_count += 1;
            }
            (Some(Ok(_)), None) => {
                return Err(IngestError::LengthMismatch {
                    messages: msg_count + 1,
                    orderbook: ob_count,
                });
            }
            (None, Some(Ok(_))) => {
                return Err(IngestError::LengthMismatch {
                    messages: msg_count,
                    orderbook: ob_count + 1,
                });
            }
            (Some(Err(e)), _) => return Err(e.into()),
            (_, Some(Err(e))) => return Err(e.into()),
        }
    }

    msg_writer.flush()?;
    ob_writer.flush()?;
    Ok((msg_count, ob_count))
}

fn open_csv_from_zip(zip_path: &Path, predicate: impl Fn(&str) -> bool) -> IngestResult<Vec<u8>> {
    let f = File::open(zip_path)?;
    let mut archive = ZipArchive::new(f)?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let name = file.name().to_string();
        if predicate(&name) {
            let mut buf = Vec::with_capacity(file.size() as usize);
            file.read_to_end(&mut buf)?;
            return Ok(buf);
        }
    }
    Err(IngestError::MissingCsv(zip_path.display().to_string()))
}

fn main() -> IngestResult<()> {
    let zips_dir = Path::new("tests/corpus/lobster/zips");
    let out_dir = Path::new("tests/corpus/lobster");
    fs::create_dir_all(out_dir)?;

    // Only process verified, listed ZIP file bases
    let file_bases = lobster_common::find_verified_zip_file_bases(
        "tests/corpus/lobster/zips",
        "tests/corpus/lobster/zips/SHASUMS.txt",
    );
    if file_bases.is_empty() {
        eprintln!("No verified LOBSTER ZIPs found. Ensure files listed in SHASUMS.txt are present and valid.");
    }

    let mut processed = 0usize;
    for base in file_bases {
        let path = zips_dir.join(format!("{}.zip", base));

        // Prefer paired ingestion (no temporary timestamp storage). Fallbacks handle single-file zips.
        let msg_csv = open_csv_from_zip(&path, |n| n.ends_with(".csv") && n.contains("_message_"));
        let ob_csv = open_csv_from_zip(&path, |n| n.ends_with(".csv") && n.contains("_orderbook_"));

        match (msg_csv, ob_csv) {
            (Ok(mb), Ok(ob)) => {
                let m_rdr = ReaderBuilder::new()
                    .has_headers(false)
                    .from_reader(mb.as_slice());
                let o_rdr = ReaderBuilder::new()
                    .has_headers(false)
                    .from_reader(ob.as_slice());
                let m_out = out_dir.join(format!("{}-message.bin", base));
                let o_out = out_dir.join(format!("{}-orderbook.bin", base));
                let (mc, oc) = write_pair_streams(m_rdr, o_rdr, &m_out, &o_out)?;
                // Write counts sidecar for benchmarks: simple, parseable text
                let counts_path = out_dir.join(format!("{}-counts.txt", base));
                let counts = format!("messages: {}\norderbook: {}\n", mc, oc);
                std::fs::write(counts_path, counts)?;
            }
            (Ok(_), Err(_)) | (Err(_), Ok(_)) => {
                return Err(IngestError::MissingPair {
                    zip: path.display().to_string(),
                });
            }
            (Err(_), Err(_)) => {
                // No relevant CSVs in this zip; skip silently
            }
        }

        processed += 1;
    }

    if processed == 0 {
        eprintln!("No ZIPs found in {}", zips_dir.display());
    }
    Ok(())
}
