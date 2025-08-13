use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::cell::{Cell, RefCell};
use std::io::Cursor;

// Import adapters and extension traits explicitly
use flatstream::framing::{DeframerExt, FramerExt, ObserverDeframer, ObserverFramer};

fn make_frame(payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4 + payload.len());
    let len = payload.len() as u32;
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(payload);
    buf
}

#[test]
fn bounded_deframer_happy_path() {
    let payload = vec![1u8, 2, 3, 4, 5];
    let data = make_frame(&payload);

    let deframer = BoundedDeframer::new(DefaultDeframer, 10);
    let mut reader = Cursor::new(&data);
    let mut buffer = Vec::new();

    let result = deframer.read_and_deframe(&mut reader, &mut buffer).unwrap();
    assert!(matches!(result, Some(())));
    assert_eq!(buffer, payload);
}

#[test]
fn bounded_deframer_over_limit() {
    let payload = vec![0u8; 16];
    let data = make_frame(&payload);

    let deframer = BoundedDeframer::new(DefaultDeframer, 8);
    let mut reader = Cursor::new(&data);
    let mut buffer = Vec::new();

    let err = deframer.read_and_deframe(&mut reader, &mut buffer).unwrap_err();
    match err {
        Error::InvalidFrame { message } => {
            assert!(message.contains("exceeds"));
        }
        other => panic!("expected InvalidFrame, got {other:?}"),
    }
}

#[test]
fn bounded_framer_happy_path() {
    let payload = b"abcde"; // 5 bytes
    let mut out = Vec::new();
    let framer = BoundedFramer::new(DefaultFramer, 10);
    framer.frame_and_write(&mut out, payload).unwrap();

    // Should be 4 bytes length + payload
    assert!(!out.is_empty());
    let mut cur = Cursor::new(&out);

    // Verify round-trip with default deframer
    let mut buf = Vec::new();
    DefaultDeframer
        .read_and_deframe(&mut cur, &mut buf)
        .unwrap()
        .unwrap();
    assert_eq!(buf, payload);
}

#[test]
fn bounded_framer_over_limit() {
    let payload = vec![7u8; 32];
    let mut out = Vec::new();
    let framer = BoundedFramer::new(DefaultFramer, 8);
    let err = framer.frame_and_write(&mut out, &payload).unwrap_err();
    match err {
        Error::InvalidFrame { message } => assert!(message.contains("exceeds")),
        other => panic!("expected InvalidFrame, got {other:?}"),
    }
}

#[test]
fn observer_framer_callback_invoked() {
    let observed_len = Cell::new(0usize);
    let payload = b"observe me";
    let framer = ObserverFramer::new(DefaultFramer, |p: &[u8]| {
        observed_len.set(p.len());
    });

    let mut out = Vec::new();
    framer.frame_and_write(&mut out, payload).unwrap();
    assert_eq!(observed_len.get(), payload.len());
}

#[test]
fn observer_deframer_callback_invoked() {
    let payload = b"hello world".to_vec();
    let data = make_frame(&payload);

    let seen: RefCell<Vec<Vec<u8>>> = RefCell::new(Vec::new());
    let deframer = ObserverDeframer::new(DefaultDeframer, |p: &[u8]| {
        seen.borrow_mut().push(p.to_vec());
    });

    let mut reader = StreamReader::new(Cursor::new(&data), deframer);
    let mut count = 0;
    reader
        .process_all(|p| {
            assert_eq!(p, &payload);
            count += 1;
            Ok(())
        })
        .unwrap();

    let seen = seen.into_inner();
    assert_eq!(count, 1);
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0], payload);
}

// --- Checksum observer tests (feature-gated) ---

#[cfg(feature = "xxhash")]
#[test]
fn observer_with_checksum_xxhash_callbacks_invoked() {
    use flatstream::XxHash64;
    use flatstream::framing::{ChecksumDeframer, ChecksumFramer};

    let payload = b"observe checksum xxhash";

    // Framer observer
    let framer_called = Cell::new(0usize);
    let framer = ObserverFramer::new(ChecksumFramer::new(XxHash64::new()), |p: &[u8]| {
        assert_eq!(p, payload);
        framer_called.set(framer_called.get() + 1);
    });
    let mut bytes = Vec::new();
    framer.frame_and_write(&mut bytes, payload).unwrap();
    assert_eq!(framer_called.get(), 1);

    // Deframer observer
    let deframer_called = Cell::new(0usize);
    let deframer = ObserverDeframer::new(ChecksumDeframer::new(XxHash64::new()), |p: &[u8]| {
        assert_eq!(p, payload);
        deframer_called.set(deframer_called.get() + 1);
    });
    let mut sr = StreamReader::new(Cursor::new(bytes), deframer);
    let mut count = 0;
    sr.process_all(|p| {
        assert_eq!(p, payload);
        count += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(count, 1);
    assert_eq!(deframer_called.get(), 1);
}

#[cfg(feature = "crc32")]
#[test]
fn observer_with_checksum_crc32_callbacks_invoked() {
    use flatstream::Crc32;
    use flatstream::framing::{ChecksumDeframer, ChecksumFramer};

    let payload = b"observe checksum crc32";

    let framer_called = Cell::new(0usize);
    let framer = ObserverFramer::new(ChecksumFramer::new(Crc32::new()), |p: &[u8]| {
        assert_eq!(p, payload);
        framer_called.set(framer_called.get() + 1);
    });
    let mut bytes = Vec::new();
    framer.frame_and_write(&mut bytes, payload).unwrap();
    assert_eq!(framer_called.get(), 1);

    let deframer_called = Cell::new(0usize);
    let deframer = ObserverDeframer::new(ChecksumDeframer::new(Crc32::new()), |p: &[u8]| {
        assert_eq!(p, payload);
        deframer_called.set(deframer_called.get() + 1);
    });
    let mut sr = StreamReader::new(Cursor::new(bytes), deframer);
    let mut count = 0;
    sr.process_all(|p| {
        assert_eq!(p, payload);
        count += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(count, 1);
    assert_eq!(deframer_called.get(), 1);
}

#[cfg(feature = "crc16")]
#[test]
fn observer_with_checksum_crc16_callbacks_invoked() {
    use flatstream::Crc16;
    use flatstream::framing::{ChecksumDeframer, ChecksumFramer};

    let payload = b"observe checksum crc16";

    let framer_called = Cell::new(0usize);
    let framer = ObserverFramer::new(ChecksumFramer::new(Crc16::new()), |p: &[u8]| {
        assert_eq!(p, payload);
        framer_called.set(framer_called.get() + 1);
    });
    let mut bytes = Vec::new();
    framer.frame_and_write(&mut bytes, payload).unwrap();
    assert_eq!(framer_called.get(), 1);

    let deframer_called = Cell::new(0usize);
    let deframer = ObserverDeframer::new(ChecksumDeframer::new(Crc16::new()), |p: &[u8]| {
        assert_eq!(p, payload);
        deframer_called.set(deframer_called.get() + 1);
    });
    let mut sr = StreamReader::new(Cursor::new(bytes), deframer);
    let mut count = 0;
    sr.process_all(|p| {
        assert_eq!(p, payload);
        count += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(count, 1);
    assert_eq!(deframer_called.get(), 1);
}

#[test]
fn fluent_bounded_equivalence_framer() {
    let payload_ok = vec![0u8; 5];
    let payload_bad = vec![0u8; 6];

    let manual = BoundedFramer::new(DefaultFramer, 5);
    let fluent = DefaultFramer.bounded(5);

    let mut a = Vec::new();
    let mut b = Vec::new();
    manual.frame_and_write(&mut a, &payload_ok).unwrap();
    fluent.frame_and_write(&mut b, &payload_ok).unwrap();
    assert_eq!(a, b);

    let err1 = manual.frame_and_write(&mut Vec::new(), &payload_bad).unwrap_err();
    let err2 = fluent.frame_and_write(&mut Vec::new(), &payload_bad).unwrap_err();
    assert!(matches!(err1, Error::InvalidFrame { .. }));
    assert!(matches!(err2, Error::InvalidFrame { .. }));
}

#[test]
fn fluent_bounded_equivalence_deframer() {
    // Create a frame with payload length 6
    let data_bad = make_frame(&vec![1u8; 6]);
    let data_ok = make_frame(&vec![2u8; 5]);

    let manual = BoundedDeframer::new(DefaultDeframer, 5);
    let fluent = DefaultDeframer.bounded(5);

    // Over-limit should error for both
    {
        let mut buf = Vec::new();
        let mut cur = Cursor::new(&data_bad);
        let err = manual
            .read_and_deframe(&mut cur, &mut buf)
            .unwrap_err();
        assert!(matches!(err, Error::InvalidFrame { .. }));
    }
    {
        let mut buf = Vec::new();
        let mut cur = Cursor::new(&data_bad);
        let err = fluent
            .read_and_deframe(&mut cur, &mut buf)
            .unwrap_err();
        assert!(matches!(err, Error::InvalidFrame { .. }));
    }

    // Under-limit should succeed for both
    {
        let mut buf = Vec::new();
        let mut cur = Cursor::new(&data_ok);
        manual.read_and_deframe(&mut cur, &mut buf).unwrap().unwrap();
        assert_eq!(buf, &data_ok[4..]);
    }
    {
        let mut buf = Vec::new();
        let mut cur = Cursor::new(&data_ok);
        fluent.read_and_deframe(&mut cur, &mut buf).unwrap().unwrap();
        assert_eq!(buf, &data_ok[4..]);
    }
}

#[test]
fn fluent_observed_equivalence_framer() {
    let payload = b"xyz";
    let mut a = Vec::new();
    let mut b = Vec::new();

    let manual = ObserverFramer::new(DefaultFramer, |_p: &[u8]| {});
    let fluent = DefaultFramer.observed(|_p: &[u8]| {});

    manual.frame_and_write(&mut a, payload).unwrap();
    fluent.frame_and_write(&mut b, payload).unwrap();

    assert_eq!(a, b);
}

#[test]
fn fluent_observed_equivalence_deframer() {
    let payload = b"observe".to_vec();
    let mut framed = Vec::new();
    DefaultFramer.frame_and_write(&mut framed, &payload).unwrap();

    let manual = ObserverDeframer::new(DefaultDeframer, |_p: &[u8]| {});
    let fluent = DefaultDeframer.observed(|_p: &[u8]| {});

    let mut buf_m = Vec::new();
    let mut cur_m = Cursor::new(&framed);
    manual.read_and_deframe(&mut cur_m, &mut buf_m).unwrap().unwrap();

    let mut buf_f = Vec::new();
    let mut cur_f = Cursor::new(&framed);
    fluent.read_and_deframe(&mut cur_f, &mut buf_f).unwrap().unwrap();

    assert_eq!(buf_m, buf_f);
}

#[test]
fn fluent_observed_callbacks_invoked() {
    // Framer: callback should see payload length
    let observed_len = Cell::new(0usize);
    let framer = DefaultFramer.observed(|p: &[u8]| observed_len.set(p.len()));
    let mut out = Vec::new();
    framer.frame_and_write(&mut out, b"hello").unwrap();
    assert_eq!(observed_len.get(), 5);

    // Deframer: callback should be called once on read
    let called = Cell::new(0usize);
    let deframer = DefaultDeframer.observed(|_p: &[u8]| called.set(called.get() + 1));
    let mut sr = StreamReader::new(Cursor::new(out), deframer);
    let mut count = 0;
    sr.process_all(|p| {
        assert_eq!(p, b"hello");
        count += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(count, 1);
    assert_eq!(called.get(), 1);
}

#[test]
fn stream_writer_with_capacity_smoke() {
    let mut sink = Vec::new();
    let writer = Cursor::new(&mut sink);
    let framer = DefaultFramer;
    let mut sw = StreamWriter::with_capacity(writer, framer, 4096);

    // Write a couple of small messages; just validate it works end-to-end
    assert!(sw.write(&"a small message").is_ok());
    assert!(sw.write(&"another one").is_ok());
    sw.flush().unwrap();

    assert!(!sw.into_inner().into_inner().is_empty());
}
#[test]
fn stream_reader_ergonomics_capacity_and_reserve() {
    let reader = Cursor::new(Vec::<u8>::new());
    let mut sr = StreamReader::with_capacity(reader, DefaultDeframer, 1024);
    assert!(sr.buffer_capacity() >= 1024);

    let target: usize = 2048;
    // Vec::reserve ensures capacity >= len + additional. With len=0, pass the absolute target.
    sr.reserve(target);
    assert!(sr.buffer_capacity() >= target);
}

#[test]
fn stream_reader_accessors_and_into_inner() {
    let reader = Cursor::new(vec![0u8; 0]);
    let mut sr = StreamReader::new(reader, DefaultDeframer);

    // Accessors compile and return references
    let _r_ref: &Cursor<Vec<u8>> = sr.get_ref();
    let _r_mut: &mut Cursor<Vec<u8>> = sr.get_mut();
    let _d_ref: &DefaultDeframer = sr.deframer();

    let _inner: Cursor<Vec<u8>> = sr.into_inner();
}

#[test]
fn stream_writer_with_builder_and_accessors() {
    let mut sink = Vec::new();
    let writer = Cursor::new(&mut sink);
    let framer = DefaultFramer;

    let builder = FlatBufferBuilder::new();
    let mut sw = StreamWriter::with_builder(writer, framer, builder);

    // Accessors compile
    let _w_ref: &Cursor<&mut Vec<u8>> = sw.get_ref();
    let _w_mut: &mut Cursor<&mut Vec<u8>> = sw.get_mut();
    let _f_ref: &DefaultFramer = sw.framer();

    // Write using expert mode with external builder
    let mut b = FlatBufferBuilder::new();
    let s = b.create_string("hello");
    b.finish(s, None);
    sw.write_finished(&mut b).unwrap();
    sw.flush().unwrap();

    assert!(!sw.into_inner().into_inner().is_empty());
}

#[test]
fn stream_writer_with_builder_alloc() {
    let mut sink = Vec::new();
    let writer = Cursor::new(&mut sink);
    let framer = DefaultFramer;

    let builder: FlatBufferBuilder = FlatBufferBuilder::new();
    let mut sw = StreamWriter::with_builder_alloc(writer, framer, builder);

    let mut b = FlatBufferBuilder::new();
    let s = b.create_string("alloc test");
    b.finish(s, None);
    sw.write_finished(&mut b).unwrap();
    sw.flush().unwrap();

    assert!(!sw.into_inner().into_inner().is_empty());
}


