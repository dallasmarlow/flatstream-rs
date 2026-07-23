use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::fs;
use std::path::PathBuf;

/// Encodes wire bytes as lowercase hex, 32 bytes (64 chars) per line, so the
/// golden corpus stays text-only in git and diffs legibly in review.
fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2 + bytes.len() / 32 + 1);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && i % 32 == 0 {
            out.push('\n');
        }
        out.push_str(&format!("{b:02x}"));
    }
    out.push('\n');
    out
}

#[test]
fn generate_corpus_files() {
    // Purpose: Generate golden corpus files (hex-encoded wire bytes) for the
    // wire format tests. Skips by default unless GENERATE_CORPUS=1 is set, so
    // an ordinary test run never rewrites the reference files.
    if std::env::var("GENERATE_CORPUS").ok().as_deref() != Some("1") {
        eprintln!("skipping corpus generation; set GENERATE_CORPUS=1 to enable");
        return;
    }

    let dir = PathBuf::from("tests/corpus");
    fs::create_dir_all(&dir).unwrap();

    let empty: &[u8] = b"";
    let small: &[u8] = b"abc";
    let mut builder = FlatBufferBuilder::with_capacity(1024);
    builder.reset();
    let data = builder.create_vector(&vec![0u8; 1024]);
    builder.finish(data, None);
    let medium: &[u8] = builder.finished_data();

    // Default framer
    for (label, payload) in [("empty", empty), ("small", small), ("medium", medium)] {
        let mut out = Vec::new();
        DefaultFramer.frame_and_write(&mut out, payload).unwrap();
        let path = dir.join(format!("default_{label}.hex"));
        fs::write(path, to_hex(&out)).unwrap();
    }

    #[cfg(feature = "xxhash")]
    {
        for (label, payload) in [("empty", empty), ("small", small), ("medium", medium)] {
            let mut out = Vec::new();
            let framer = framing::ChecksumFramer::new(XxHash64::new());
            framer.frame_and_write(&mut out, payload).unwrap();
            let path = dir.join(format!("xxhash64_{label}.hex"));
            fs::write(path, to_hex(&out)).unwrap();
        }
    }

    #[cfg(feature = "crc32")]
    {
        for (label, payload) in [("empty", empty), ("small", small), ("medium", medium)] {
            let mut out = Vec::new();
            let framer = framing::ChecksumFramer::new(Crc32::new());
            framer.frame_and_write(&mut out, payload).unwrap();
            let path = dir.join(format!("crc32_{label}.hex"));
            fs::write(path, to_hex(&out)).unwrap();
        }
    }

    #[cfg(feature = "crc16")]
    {
        for (label, payload) in [("empty", empty), ("small", small), ("medium", medium)] {
            let mut out = Vec::new();
            let framer = framing::ChecksumFramer::new(Crc16::new());
            framer.frame_and_write(&mut out, payload).unwrap();
            let path = dir.join(format!("crc16_{label}.hex"));
            fs::write(path, to_hex(&out)).unwrap();
        }
    }
}
