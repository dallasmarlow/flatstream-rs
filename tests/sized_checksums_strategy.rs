#![cfg(any(feature = "xxhash", feature = "crc32", feature = "crc16"))]

use flatstream::*;
mod test_harness;
use test_harness::TestHarness;

#[test]
fn sized_checksums_independent_streams() {
    // Purpose: Validate each checksum strategy independently, mirroring intended
    // usage where a stream does not switch formats mid-stream.

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
