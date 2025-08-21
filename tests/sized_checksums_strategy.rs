#![cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]

use flatstream::*;
mod test_harness;
use test_harness::TestHarness;

#[test]
fn sized_checksums_independent_streams() {
    // Each checksum strategy validated on an independent stream.
    // This reflects intended usage (no implicit format switching mid-stream).

    #[cfg(feature = "crc16")]
    {
        let harness = TestHarness::new();
        let mut w = harness.writer(ChecksumFramer::new(Crc16::new()));
        assert!(w.write(&"small").is_ok());
        w.flush().unwrap();
        let mut r = harness.reader(ChecksumDeframer::new(Crc16::new()));
        assert!(r.read_message().unwrap().is_some());
    }

    #[cfg(feature = "crc32")]
    {
        let harness = TestHarness::new();
        let mut w = harness.writer(ChecksumFramer::new(Crc32::new()));
        assert!(w.write(&"medium_message").is_ok());
        w.flush().unwrap();
        let mut r = harness.reader(ChecksumDeframer::new(Crc32::new()));
        assert!(r.read_message().unwrap().is_some());
    }

    #[cfg(feature = "xxhash")]
    {
        let harness = TestHarness::new();
        let mut w = harness.writer(ChecksumFramer::new(XxHash64::new()));
        let payload = String::from_utf8(vec![b'x'; 1024]).unwrap();
        assert!(w.write(&payload).is_ok());
        w.flush().unwrap();
        let mut r = harness.reader(ChecksumDeframer::new(XxHash64::new()));
        assert!(r.read_message().unwrap().is_some());
    }
}
