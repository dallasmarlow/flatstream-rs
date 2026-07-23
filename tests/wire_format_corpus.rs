//! Golden-file tests: the wire format must stay byte-stable across versions.
//!
//! The corpus files are checked in as hex-encoded text (`tests/corpus/*.hex`)
//! so the repository stays free of binary files while the exact wire bytes
//! remain pinned; a MISSING file is a failure, not a skip — a silently-skipping
//! golden test is how wire breaks ship. After an *intentional* format change,
//! regenerate deliberately and commit the new bytes:
//!
//!   GENERATE_CORPUS=1 cargo test --features all_checksums --test generate_corpus
//!
//! Each test asserts two things per payload: framing the canonical payload
//! today reproduces the committed bytes exactly (byte-golden), and the
//! committed bytes still roundtrip through today's reader.

use flatstream::*;
use std::fs;
use std::io::Cursor;

/// Reads a hex-encoded golden file back into the exact wire bytes it pins.
fn golden(path: &str) -> Vec<u8> {
    let text = fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "golden corpus file {path} unreadable ({e}); regenerate with \
             GENERATE_CORPUS=1 cargo test --features all_checksums --test generate_corpus \
             and commit the result"
        )
    });
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    assert!(
        digits.len().is_multiple_of(2),
        "golden corpus file {path} is not valid hex (odd digit count)"
    );
    digits
        .chunks_exact(2)
        .map(|pair| {
            let hi = (pair[0] as char).to_digit(16);
            let lo = (pair[1] as char).to_digit(16);
            match (hi, lo) {
                (Some(hi), Some(lo)) => (hi * 16 + lo) as u8,
                _ => panic!("golden corpus file {path} contains a non-hex digit"),
            }
        })
        .collect()
}

/// The canonical corpus payloads. Must stay in sync with
/// `tests/generate_corpus.rs` — the byte-golden assertion fails loudly if the
/// two drift apart.
fn payloads() -> Vec<(&'static str, Vec<u8>)> {
    let mut builder = flatbuffers::FlatBufferBuilder::with_capacity(1024);
    builder.reset();
    let data = builder.create_vector(&vec![0u8; 1024]);
    builder.finish(data, None);
    vec![
        ("empty", Vec::new()),
        ("small", b"abc".to_vec()),
        ("medium", builder.finished_data().to_vec()),
    ]
}

fn assert_golden<F: Framer, D: Deframer>(prefix: &str, framer: &F, make_deframer: impl Fn() -> D) {
    for (label, payload) in payloads() {
        let bytes = golden(&format!("tests/corpus/{prefix}_{label}.hex"));

        // Byte-golden: today's framer must reproduce the committed bytes.
        let mut reframed = Vec::new();
        framer.frame_and_write(&mut reframed, &payload).unwrap();
        assert_eq!(bytes, reframed, "wire format drift for {prefix}/{label}");

        // The committed bytes still roundtrip through today's reader.
        let mut reader = StreamReader::new(Cursor::new(&bytes), make_deframer());
        let got = reader.read_message().unwrap().expect("one frame");
        assert_eq!(got, &payload[..], "payload drift for {prefix}/{label}");
        assert!(reader.read_message().unwrap().is_none());
    }
}

/// Reading a checksummed stream with the plain deframer yields a frame of the
/// declared length whose bytes are wrong (checksum prefix + truncated payload)
/// — silent mis-framing, not an error.
#[cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]
fn assert_cross_strategy_misframe(prefix: &str) {
    let bytes = golden(&format!("tests/corpus/{prefix}_medium.hex"));
    let (_, payload) = payloads().pop().expect("medium is last");
    let mut r = StreamReader::new(Cursor::new(&bytes), DefaultDeframer::new());
    let got = r.read_message().unwrap().expect("mis-framed but parseable");
    assert_eq!(got.len(), payload.len(), "length field is honored");
    assert_ne!(got, &payload[..], "bytes are silently wrong for {prefix}");
}

#[test]
fn corpus_default_golden() {
    assert_golden("default", &DefaultFramer, DefaultDeframer::new);
}

#[cfg(feature = "xxhash")]
#[test]
fn corpus_xxhash64_golden() {
    assert_golden(
        "xxhash64",
        &framing::ChecksumFramer::new(XxHash64::new()),
        || framing::ChecksumDeframer::new(XxHash64::new()),
    );
    // Mismatched strategies do NOT fail — they silently mis-frame: the default
    // deframer reads the length, then consumes [checksum | payload-prefix] as a
    // structurally valid frame of the right size with the wrong bytes. (The
    // original version of this test asserted `is_err()` and "passed" for years
    // because the corpus files were gitignored and the test silently skipped;
    // the first real run disproved it.) This is exactly why strategy agreement
    // is out-of-band and why integrity requires the checksum to be enabled.
    assert_cross_strategy_misframe("xxhash64");
}

#[cfg(feature = "crc32")]
#[test]
fn corpus_crc32_golden() {
    assert_golden("crc32", &framing::ChecksumFramer::new(Crc32::new()), || {
        framing::ChecksumDeframer::new(Crc32::new())
    });
    assert_cross_strategy_misframe("crc32");
}

#[cfg(feature = "crc16")]
#[test]
fn corpus_crc16_golden() {
    assert_golden("crc16", &framing::ChecksumFramer::new(Crc16::new()), || {
        framing::ChecksumDeframer::new(Crc16::new())
    });
    assert_cross_strategy_misframe("crc16");
}
