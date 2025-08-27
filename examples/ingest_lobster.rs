use anyhow::{anyhow, Context, Result};
use csv::{ReaderBuilder, StringRecord};
use flatbuffers::FlatBufferBuilder;
use flatstream::{DefaultFramer, StreamWriter};
use std::fs::{self, File};
use std::io::{BufWriter, Read};
use std::path::Path;
use zip::read::ZipArchive;

// Include checked-in generated code so no build-time codegen is required.
// Generate once with:
// flatc --rust -o examples/generated examples/schemas/lobster_message.fbs examples/schemas/lobster_orderbook.fbs
#[allow(dead_code)]
mod lobster_generated {
    pub mod message {
        include!("generated/lobster_message_generated.rs");
        pub use self::flatstream::lobster::*;
    }
    pub mod orderbook {
        include!("generated/lobster_orderbook_generated.rs");
        pub use self::flatstream::lobster::*;
    }
}

struct MessageRow {
    timestamp: f64,
    event_type: u8,
    order_id: u64,
    size: u32,
    price: i64,
    direction: i8,
}

fn parse_message_row(rec: &StringRecord) -> Result<MessageRow> {
    let timestamp: f64 = rec.get(0).ok_or_else(|| anyhow!("missing time"))?.parse()?;
    let event_type: u8 = rec.get(1).ok_or_else(|| anyhow!("missing type"))?.parse()?;
    let order_id: u64 = rec
        .get(2)
        .ok_or_else(|| anyhow!("missing order_id"))?
        .parse()?;
    let size: u32 = rec.get(3).ok_or_else(|| anyhow!("missing size"))?.parse()?;
    let price_scaled: i64 = rec
        .get(4)
        .ok_or_else(|| anyhow!("missing price"))?
        .parse()?; // already x10000
    let direction: i8 = rec
        .get(5)
        .ok_or_else(|| anyhow!("missing direction"))?
        .parse()?;

    Ok(MessageRow {
        timestamp,
        event_type,
        order_id,
        size,
        price: price_scaled,
        direction,
    })
}

fn write_message_stream<R: Read>(mut rdr: csv::Reader<R>, output_path: &Path) -> Result<Vec<f64>> {
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

fn write_orderbook_stream<R: Read>(
    mut rdr: csv::Reader<R>,
    output_path: &Path,
    message_timestamps: Option<&[f64]>,
) -> Result<()> {
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

fn open_csv_from_zip(zip_path: &Path, predicate: impl Fn(&str) -> bool) -> Result<Vec<u8>> {
    let f = File::open(zip_path).with_context(|| format!("open {}", zip_path.display()))?;
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
    Err(anyhow!("no matching CSV entry in {}", zip_path.display()))
}

fn main() -> Result<()> {
    let zips_dir = Path::new("tests/corpus/lobster/zips");
    let out_dir = Path::new("tests/corpus/lobster");
    fs::create_dir_all(out_dir)?;

    let mut processed = 0usize;
    for entry in fs::read_dir(zips_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("zip") {
            continue;
        }

        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("dataset");

        // message stream (match any *_message_*.csv)
        let message_timestamps: Option<Vec<f64>> = if let Ok(csv_bytes) =
            open_csv_from_zip(&path, |n| n.ends_with(".csv") && n.contains("_message_"))
        {
            let rdr = ReaderBuilder::new()
                .has_headers(false)
                .from_reader(csv_bytes.as_slice());
            let out = out_dir.join(format!("{}-message.bin", stem));
            Some(write_message_stream(rdr, &out)?)
        } else {
            None
        };

        // orderbook stream (match any *_orderbook_*.csv)
        if let Ok(csv_bytes) =
            open_csv_from_zip(&path, |n| n.ends_with(".csv") && n.contains("_orderbook_"))
        {
            let rdr = ReaderBuilder::new()
                .has_headers(false)
                .from_reader(csv_bytes.as_slice());
            let out = out_dir.join(format!("{}-orderbook.bin", stem));
            let mt = message_timestamps.as_deref();
            write_orderbook_stream(rdr, &out, mt)?;
        }

        processed += 1;
    }

    if processed == 0 {
        eprintln!("No ZIPs found in {}", zips_dir.display());
    }
    Ok(())
}
