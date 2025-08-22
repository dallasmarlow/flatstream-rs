use flatstream::framing::{DeframerExt, FramerExt};
use flatstream::*;
use proptest::prelude::*;
use std::io::Cursor;

// Typed reading property tests using simple string roots
struct StrRoot;

impl<'a> StreamDeserialize<'a> for StrRoot {
    type Root = &'a str;
    fn from_payload(payload: &'a [u8]) -> Result<Self::Root> {
        flatbuffers::root::<&'a str>(payload).map_err(Error::FlatbuffersError)
    }
}

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

// Additional typed read properties
proptest! {
    #[test]
    fn typed_roundtrip_strings(ref s in "[a-zA-Z0-9 _-]{0,128}") {
        // frame
        let mut out = Vec::new();
        let mut builder = flatbuffers::FlatBufferBuilder::new();
        let str_off = builder.create_string(s);
        builder.finish(str_off, None);
        DefaultFramer.frame_and_write(&mut out, builder.finished_data()).unwrap();

        // deframe and typed process
        let mut reader = StreamReader::new(Cursor::new(&out), DefaultDeframer);
        let mut seen = None;
        reader.process_typed::<StrRoot, _>(|root| { seen = Some(root.to_string()); Ok(()) }).unwrap();
        prop_assert_eq!(seen.as_deref(), Some(&s[..]));
    }
}
