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
