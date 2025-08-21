use flatstream::framing::{DeframerExt, FramerExt};
use flatstream::*;
use proptest::prelude::*;
use std::io::Cursor;

proptest! {
    #[test]
    fn roundtrip_default_deframer(ref data in proptest::collection::vec(any::<u8>(), 0..1024)) {
        // frame
        let mut out = Vec::new();
        DefaultFramer.frame_and_write(&mut out, data).unwrap();
        // deframe
        let mut buf = Vec::new();
        let mut cur = Cursor::new(&out);
        DefaultDeframer.read_and_deframe(&mut cur, &mut buf).unwrap().unwrap();
        prop_assert_eq!(&buf, data);
    }
}

const MAX_PROPTEST_PAYLOAD_SIZE: usize = 2048;

proptest! {
    #[test]
    fn bounded_roundtrip(ref data in proptest::collection::vec(any::<u8>(), 0..MAX_PROPTEST_PAYLOAD_SIZE)) {
        let limit = MAX_PROPTEST_PAYLOAD_SIZE + 1;
        let framer = DefaultFramer.bounded(limit);
        let deframer = DefaultDeframer.bounded(limit);

        // frame
        let mut out = Vec::new();
        framer.frame_and_write(&mut out, data).unwrap();

        // deframe
        let mut buf = Vec::new();
        let mut cur = Cursor::new(&out);
        deframer.read_and_deframe(&mut cur, &mut buf).unwrap().unwrap();
        prop_assert_eq!(&buf, data);
    }

    #[cfg(feature="crc32")]
    #[test]
    fn checksum_crc32_roundtrip(ref data in proptest::collection::vec(any::<u8>(), 0..MAX_PROPTEST_PAYLOAD_SIZE)) {
        let framer = ChecksumFramer::new(Crc32::new());
        let deframer = ChecksumDeframer::new(Crc32::new());

        let mut out = Vec::new();
        framer.frame_and_write(&mut out, data).unwrap();

        let mut buf = Vec::new();
        let mut cur = Cursor::new(&out);
        deframer.read_and_deframe(&mut cur, &mut buf).unwrap().unwrap();
        prop_assert_eq!(&buf, data);
    }
}
