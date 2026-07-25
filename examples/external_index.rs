// Example purpose: Build an external "offset -> frame" index using FrameReceipt,
// then random-access individual frames by seeking a reader to a recorded offset.
// This is the pattern a terminal scrollback journal (or any sparse index) needs,
// and it replaces the fragile `bytes += 8 + payload_len` arithmetic a consumer
// would otherwise have to keep in lockstep with the wire layout.
//!
//! Demonstrates `write_with_receipt` / `FrameReceipt` for index construction and
//! caller-scratch `read_frame_at` for allocation-free steady-state lookup.

use flatstream::{
    read_frame_at, DefaultDeframer, DefaultFramer, FrameReceipt, Result, StreamWriter,
};
use std::io::Cursor;

/// One index entry: the application's record id and where its frame lives.
struct IndexEntry {
    id: u32,
    receipt: FrameReceipt,
}

fn main() -> Result<()> {
    let records = [
        (10u32, "first record"),
        (20, "second record — a bit longer"),
        (30, "third"),
        (40, "fourth record payload"),
    ];

    // --- Write pass: capture a receipt per frame into an external index. ---
    println!(
        "[write] framing {} records, recording an offset index",
        records.len()
    );
    let mut wire: Vec<u8> = Vec::new();
    let mut index: Vec<IndexEntry> = Vec::new();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut wire), DefaultFramer);
        for (id, text) in records {
            // The receipt tells us exactly where this frame landed — no
            // knowledge of the header or wire layout in this code.
            let receipt = writer.write_with_receipt(&text)?;
            index.push(IndexEntry { id, receipt });
        }
        writer.flush()?;

        // `bytes_written()` is the running stream offset: it must equal the
        // total bytes produced.
        assert_eq!(writer.bytes_written() as usize, wire.len());
    }

    // The index must tile the stream: each frame begins exactly where the
    // previous one ended, and together they cover every byte.
    let mut expected_start = 0u64;
    for entry in &index {
        assert_eq!(entry.receipt.frame_start, expected_start);
        expected_start += entry.receipt.wire_len;
    }
    assert_eq!(expected_start as usize, wire.len());
    println!(
        "[index] {} entries, contiguous and covering {} bytes",
        index.len(),
        wire.len()
    );

    // --- Random access: fetch frames out of order by seeking to the offset. ---
    // Fetch id 30 (the 3rd record) and id 10 (the 1st), in that order.
    let mut source = Cursor::new(&wire);
    let mut scratch = Vec::new();
    for wanted in [30u32, 10] {
        let entry = index
            .iter()
            .find(|e| e.id == wanted)
            .expect("record present in index");

        let frame = read_frame_at(
            &mut source,
            &DefaultDeframer::new(),
            entry.receipt.frame_start,
            &mut scratch,
        )?
        .expect("a frame begins at the indexed offset");
        assert_eq!(frame.receipt, entry.receipt);
        let text = flatbuffers::root::<&str>(frame.payload).expect("string root");

        let (_, expected) = records
            .iter()
            .copied()
            .find(|(id, _)| *id == wanted)
            .unwrap();
        assert_eq!(text, expected);
        println!(
            "[fetch] id {wanted} @ offset {} -> {text:?}",
            entry.receipt.frame_start
        );
    }

    println!("[ok] external index built from receipts; random access verified");
    Ok(())
}
