// Example purpose: Choosing a checksum width by payload size and integrity
// requirements — with the wire format proven from observed bytes, not asserted
// in prose. For each enabled algorithm this example writes frames of genuinely
// different sizes (16 B / 1 KiB / 64 KiB), derives the per-frame overhead from
// the actual output length, and asserts it equals `4 + Checksum::SIZE` exactly.
// It ends by corrupting a payload byte and proving detection surfaces as
// `ErrorKind::ChecksumMismatch`.
//
// Rule of thumb the numbers make concrete: overhead matters only relative to
// payload size — and it is the whole header (length + checksum), not the
// checksum alone. The printed percentages divide by the *measured FlatBuffers
// payload* bytes (application blob + builder overhead: root offset + vector
// length), not by the logical blob size — on the smallest frames the header
// is a double-digit percentage, on 64 KiB batches it rounds to ~0%. So small,
// lossy-tolerant frames earn narrow checksums, large or critical frames take
// the strongest one, and the *stream* (not the frame) fixes the choice.
// (Two earlier drafts of this comment quoted wrong percentages — checksum-only
// first, then blob-relative; the measured output kept correcting the prose,
// which is the point of this example.)

use flatstream::*;
use std::io::Cursor;

#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
use flatstream::checksum::Checksum;
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
use flatstream::framing::{ChecksumDeframer, ChecksumFramer};

/// Serializes a payload of exactly `bytes` length (a FlatBuffers byte vector
/// costs a small constant on top; we assert on *deltas*, so it cancels out).
struct Blob(Vec<u8>);

impl StreamSerialize for Blob {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<A>,
    ) -> Result<()> {
        let v = builder.create_vector(&self.0);
        builder.finish(v, None);
        Ok(())
    }
}

const SIZES: &[(&str, usize)] = &[
    ("small control frame", 16),
    ("medium batch", 1024),
    ("large batch", 64 * 1024),
];

/// Writes one frame of each size with `framer`, reads them back with
/// `deframer`, and returns the measured per-frame header overhead (identical
/// for every frame: wire_len − payload_len).
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
fn measure_overhead<C: Checksum + Copy>(alg: C, name: &str) -> Result<usize> {
    let mut wire = Vec::new();
    // Exact FlatBuffers payload length per frame — note this is larger than
    // the application blob (the builder adds a root offset + vector length),
    // so overhead percentages below divide by *payload* bytes, not blob bytes.
    let mut payload_lens = Vec::new();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut wire), ChecksumFramer::new(alg));
        for &(_, size) in SIZES {
            let mut b = flatbuffers::FlatBufferBuilder::new();
            Blob(vec![0xA5; size]).serialize(&mut b)?;
            payload_lens.push(b.finished_data().len());
            writer.write(&Blob(vec![0xA5; size]))?;
        }
        writer.flush()?;
    }

    // Read back and verify integrity end to end.
    let mut reader = StreamReader::new(Cursor::new(&wire), ChecksumDeframer::new(alg));
    let mut frames = 0;
    reader.process_all(|_| {
        frames += 1;
        Ok(())
    })?;
    assert_eq!(frames, SIZES.len());

    let payload_total: usize = payload_lens.iter().sum();
    let overhead_per_frame = (wire.len() - payload_total) / SIZES.len();
    let smallest = payload_lens[0] as f64;
    let largest = payload_lens[payload_lens.len() - 1] as f64;
    println!(
        "{name:<10} SIZE={}  measured overhead: {overhead_per_frame} B/frame  \
         (smallest payload, {} B: {:.1}%; largest, {} B: {:.3}%)",
        C::SIZE,
        payload_lens[0],
        overhead_per_frame as f64 * 100.0 / smallest,
        payload_lens[payload_lens.len() - 1],
        overhead_per_frame as f64 * 100.0 / largest,
    );

    // The wire format, proven from observed bytes: [4-byte len | SIZE | payload].
    assert_eq!(overhead_per_frame, 4 + C::SIZE);
    Ok(overhead_per_frame)
}

fn main() -> Result<()> {
    println!("=== Sized Checksums: overhead measured, format proven ===\n");

    // Baseline without a checksum: overhead is exactly the 4-byte length.
    {
        let mut wire = Vec::new();
        let mut payload_total = 0usize;
        {
            let mut writer = StreamWriter::new(Cursor::new(&mut wire), DefaultFramer);
            for &(_, size) in SIZES {
                let mut b = flatbuffers::FlatBufferBuilder::new();
                Blob(vec![0xA5; size]).serialize(&mut b)?;
                payload_total += b.finished_data().len();
                writer.write(&Blob(vec![0xA5; size]))?;
            }
            writer.flush()?;
        }
        let overhead = (wire.len() - payload_total) / SIZES.len();
        println!("no checksum  measured overhead: {overhead} B/frame");
        assert_eq!(overhead, 4);
    }

    #[cfg(feature = "crc16")]
    measure_overhead(Crc16::new(), "CRC16")?;
    #[cfg(feature = "crc32")]
    measure_overhead(Crc32::new(), "CRC32")?;
    #[cfg(feature = "xxhash")]
    measure_overhead(XxHash64::new(), "XXH3-64")?;

    // Corruption detection: flip one payload byte, read must fail with
    // ChecksumMismatch — integrity is the reason the wider field is worth it.
    #[cfg(feature = "xxhash")]
    {
        let mut wire = Vec::new();
        let mut writer =
            StreamWriter::new(Cursor::new(&mut wire), ChecksumFramer::new(XxHash64::new()));
        writer.write(&Blob(vec![0xA5; 1024]))?;
        writer.flush()?;
        drop(writer);

        let last = wire.len() - 1;
        wire[last] ^= 0x01; // corrupt the final payload byte

        let mut reader =
            StreamReader::new(Cursor::new(&wire), ChecksumDeframer::new(XxHash64::new()));
        let err = reader.read_message().unwrap_err();
        assert!(matches!(err.kind(), ErrorKind::ChecksumMismatch { .. }));
        println!("\ncorrupted one byte → {err}");
    }

    #[cfg(not(any(feature = "xxhash", feature = "crc32", feature = "crc16")))]
    println!("(enable checksum features to see the per-algorithm measurements:");
    #[cfg(not(any(feature = "xxhash", feature = "crc32", feature = "crc16")))]
    println!(" cargo run --example sized_checksums_example --features all_checksums)");

    println!("\nall assertions passed — wire format proven from observed bytes ✓");
    Ok(())
}
