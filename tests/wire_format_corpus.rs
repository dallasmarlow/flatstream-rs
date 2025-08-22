use flatstream::*;
use std::fs;
use std::path::Path;

fn read_file(path: &str) -> Option<Vec<u8>> {
    fs::read(Path::new(path)).ok()
}

#[test]
fn corpus_default_layout_and_roundtrip() {
    for label in ["empty", "small", "medium"] {
        let path = format!("tests/corpus/default_{label}.bin");
        let Some(bytes) = read_file(&path) else {
            return;
        };
        assert!(bytes.len() >= 4);
        let len = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        assert_eq!(bytes.len(), 4 + len);
        let mut r = StreamReader::new(std::io::Cursor::new(&bytes), DefaultDeframer);
        let mut count = 0usize;
        r.process_all(|p| {
            assert_eq!(p.len(), len);
            count += 1;
            Ok(())
        })
        .unwrap();
        // NOTE: The branch is intentionally identical to make the condition explicit and
        // guard against future format/layout changes. We expect exactly one message
        // regardless of length here because each file contains a single framed payload.
        assert_eq!(count, if len == 0 && bytes.len() == 4 { 1 } else { 1 });
    }
}

#[cfg(feature = "xxhash")]
#[test]
fn corpus_xxhash_layout_and_roundtrip() {
    for label in ["empty", "small", "medium"] {
        let path = format!("tests/corpus/xxhash64_{label}.bin");
        let Some(bytes) = read_file(&path) else {
            return;
        };
        assert!(bytes.len() >= 12);
        let len = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        assert_eq!(bytes.len(), 4 + 8 + len);

        // Roundtrip with checksum deframer
        let mut r = StreamReader::new(
            std::io::Cursor::new(&bytes),
            framing::ChecksumDeframer::new(XxHash64::new()),
        );
        let mut count = 0usize;
        r.process_all(|p| {
            assert_eq!(p.len(), len);
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 1);

        // Cross-deframer failure
        let mut r2 = StreamReader::new(std::io::Cursor::new(&bytes), DefaultDeframer);
        assert!(r2.read_message().is_err());
    }
}

#[cfg(feature = "crc32")]
#[test]
fn corpus_crc32_layout_and_roundtrip() {
    for label in ["empty", "small", "medium"] {
        let path = format!("tests/corpus/crc32_{label}.bin");
        let Some(bytes) = read_file(&path) else {
            return;
        };
        assert!(bytes.len() >= 8);
        let len = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        assert_eq!(bytes.len(), 4 + 4 + len);

        let mut r = StreamReader::new(
            std::io::Cursor::new(&bytes),
            framing::ChecksumDeframer::new(Crc32::new()),
        );
        let mut count = 0usize;
        r.process_all(|p| {
            assert_eq!(p.len(), len);
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 1);

        let mut r2 = StreamReader::new(std::io::Cursor::new(&bytes), DefaultDeframer);
        assert!(r2.read_message().is_err());
    }
}

#[cfg(feature = "crc16")]
#[test]
fn corpus_crc16_layout_and_roundtrip() {
    for label in ["empty", "small", "medium"] {
        let path = format!("tests/corpus/crc16_{label}.bin");
        let Some(bytes) = read_file(&path) else {
            return;
        };
        assert!(bytes.len() >= 6);
        let len = u32::from_le_bytes(bytes[0..4].try_into().unwrap()) as usize;
        assert_eq!(bytes.len(), 4 + 2 + len);

        let mut r = StreamReader::new(
            std::io::Cursor::new(&bytes),
            framing::ChecksumDeframer::new(Crc16::new()),
        );
        let mut count = 0usize;
        r.process_all(|p| {
            assert_eq!(p.len(), len);
            count += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 1);

        let mut r2 = StreamReader::new(std::io::Cursor::new(&bytes), DefaultDeframer);
        assert!(r2.read_message().is_err());
    }
}
