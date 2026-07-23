use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::cell::{Cell, RefCell};

// Import adapters and extension traits explicitly
use flatstream::framing::{DeframerExt, FramerExt, ObserverDeframer, ObserverFramer};
use std::io::Cursor;

fn make_frame(payload: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4 + payload.len());
    let len = payload.len() as u32;
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(payload);
    buf
}

#[test]
fn bounded_deframer_happy_path() {
    // Purpose: A deframer bound should accept a frame under the limit and return the payload.
    let payload = vec![1u8, 2, 3, 4, 5];
    let data = make_frame(&payload);

    let deframer = DefaultDeframer::new().with_max_frame_len(10);
    let mut reader = Cursor::new(&data);
    let mut buffer = Vec::new();

    let n = deframer
        .read_and_deframe(&mut reader, &mut buffer)
        .unwrap()
        .unwrap();
    assert_eq!(&buffer[..n], payload);
}

#[test]
fn bounded_deframer_over_limit() {
    // Purpose: The deframer must reject frames whose declared length exceeds the bound.
    let payload = vec![0u8; 16];
    let data = make_frame(&payload);

    let deframer = DefaultDeframer::new().with_max_frame_len(8);
    let mut reader = Cursor::new(&data);
    let mut buffer = Vec::new();

    let err = deframer
        .read_and_deframe(&mut reader, &mut buffer)
        .unwrap_err();
    match err.into_kind() {
        ErrorKind::InvalidFrame { message, .. } => {
            assert!(message.contains("exceeds"));
        }
        other => panic!("expected InvalidFrame, got {other:?}"),
    }
}

#[test]
fn bounded_framer_happy_path() {
    // Purpose: BoundedFramer should allow payloads under the limit and support round-trip.
    let payload = b"abcde"; // 5 bytes
    let mut out = Vec::new();
    let framer = BoundedFramer::new(DefaultFramer, 10);
    framer.frame_and_write(&mut out, payload).unwrap();

    // Should be 4 bytes length + payload
    assert!(!out.is_empty());
    let mut cur = Cursor::new(&out);

    // Verify round-trip with default deframer
    let mut buf = Vec::new();
    let n = DefaultDeframer::new()
        .read_and_deframe(&mut cur, &mut buf)
        .unwrap()
        .unwrap();
    assert_eq!(&buf[..n], payload);
}

#[test]
fn write_over_limit_returns_invalid_frame() {
    // Purpose: Writing over the bound returns InvalidFrame with the declared
    // length and limit in context — asserted exactly, not via panic-message
    // substring matching.
    let mut out = Vec::new();
    let framer = DefaultFramer.bounded(4);
    // 5 bytes exceeds the 4-byte bound.
    let err = framer.frame_and_write(&mut out, b"hello").unwrap_err();
    match err.into_kind() {
        ErrorKind::InvalidFrame {
            declared_len,
            limit,
            ..
        } => {
            assert_eq!(declared_len, Some(5));
            assert_eq!(limit, Some(4));
        }
        other => panic!("expected InvalidFrame, got {other:?}"),
    }
    assert!(out.is_empty(), "nothing may be written on rejection");
}

#[test]
fn observer_framer_callback_invoked() {
    // Purpose: ObserverFramer invokes the callback with the payload on write.
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
    // Purpose: ObserverDeframer invokes the callback with the payload on read.
    let payload = b"hello world".to_vec();
    let data = make_frame(&payload);

    let seen: RefCell<Vec<Vec<u8>>> = RefCell::new(Vec::new());
    let deframer = ObserverDeframer::new(DefaultDeframer::new(), |p: &[u8]| {
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

// --- Checksum observer test (feature-gated) ---
// One checksummed variant suffices: the observer adapters are generic over the
// inner framer/deframer, so composing with any checksum proves the forwarding;
// the algorithms themselves are covered by the checksum/wire-format tests.

#[cfg(feature = "xxhash")]
#[test]
fn observer_with_checksum_callbacks_invoked() {
    // Purpose: Observer callbacks fire correctly composed over checksum framing.
    use flatstream::framing::{ChecksumDeframer, ChecksumFramer};
    use flatstream::XxHash64;

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

#[test]
fn fluent_bounded_equivalence_framer() {
    // Purpose: Fluent API .bounded() produces identical behavior to manual BoundedFramer.
    let payload_ok = vec![0u8; 5];
    let payload_bad = vec![0u8; 6];

    let manual = BoundedFramer::new(DefaultFramer, 5);
    let fluent = DefaultFramer.bounded(5);

    let mut a = Vec::new();
    let mut b = Vec::new();
    manual.frame_and_write(&mut a, &payload_ok).unwrap();
    fluent.frame_and_write(&mut b, &payload_ok).unwrap();
    assert_eq!(a, b);

    let err1 = manual
        .frame_and_write(&mut Vec::new(), &payload_bad)
        .unwrap_err();
    let err2 = fluent
        .frame_and_write(&mut Vec::new(), &payload_bad)
        .unwrap_err();
    assert!(matches!(err1.kind(), ErrorKind::InvalidFrame { .. }));
    assert!(matches!(err2.kind(), ErrorKind::InvalidFrame { .. }));
}

#[test]
fn max_frame_len_enforced_through_adapters() {
    // Purpose: The core deframer's bound is enforced on both trait entry points,
    // including when composed under an adapter (which forwards read_after_length).
    let data_bad = make_frame(&[1u8; 6]);
    let data_ok = make_frame(&[2u8; 5]);

    let deframer = DefaultDeframer::new()
        .with_max_frame_len(5)
        .observed(|_p: &[u8]| {});

    // Over-limit should error through the adapter
    {
        let mut buf = Vec::new();
        let mut cur = Cursor::new(&data_bad);
        let err = deframer.read_and_deframe(&mut cur, &mut buf).unwrap_err();
        assert!(matches!(err.kind(), ErrorKind::InvalidFrame { .. }));
    }

    // Under-limit should succeed through the adapter
    {
        let mut buf = Vec::new();
        let mut cur = Cursor::new(&data_ok);
        let n = deframer
            .read_and_deframe(&mut cur, &mut buf)
            .unwrap()
            .unwrap();
        assert_eq!(&buf[..n], &data_ok[4..]);
    }
}

#[test]
fn fluent_observed_equivalence_framer() {
    // Purpose: Fluent API .observed() produces identical output bytes to manual ObserverFramer.
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
    // Purpose: Fluent API .observed() produces payloads identical to manual ObserverDeframer.
    let payload = b"observe".to_vec();
    let mut framed = Vec::new();
    DefaultFramer
        .frame_and_write(&mut framed, &payload)
        .unwrap();

    let manual = ObserverDeframer::new(DefaultDeframer::new(), |_p: &[u8]| {});
    let fluent = DefaultDeframer::new().observed(|_p: &[u8]| {});

    let mut buf_m = Vec::new();
    let mut cur_m = Cursor::new(&framed);
    manual
        .read_and_deframe(&mut cur_m, &mut buf_m)
        .unwrap()
        .unwrap();

    let mut buf_f = Vec::new();
    let mut cur_f = Cursor::new(&framed);
    fluent
        .read_and_deframe(&mut cur_f, &mut buf_f)
        .unwrap()
        .unwrap();

    assert_eq!(buf_m, buf_f);
}

#[test]
fn fluent_observed_callbacks_invoked() {
    // Purpose: Fluent observed adapters invoke callbacks once per write/read with expected data.
    // Framer: callback should see payload length
    let observed_len = Cell::new(0usize);
    let framer = DefaultFramer.observed(|p: &[u8]| observed_len.set(p.len()));
    let mut out = Vec::new();
    framer.frame_and_write(&mut out, b"hello").unwrap();
    assert_eq!(observed_len.get(), 5);

    // Deframer: callback should be called once on read
    let called = Cell::new(0usize);
    let deframer = DefaultDeframer::new().observed(|_p: &[u8]| called.set(called.get() + 1));
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
    let mut sr = StreamReader::with_capacity(reader, DefaultDeframer::new(), 1024);
    assert!(sr.buffer_capacity() >= 1024);

    let target: usize = 2048;
    // Vec::reserve ensures capacity >= len + additional. With len=0, pass the absolute target.
    sr.reserve(target);
    assert!(sr.buffer_capacity() >= target);
}

#[test]
fn stream_reader_accessors_and_into_inner() {
    let reader = Cursor::new(vec![0u8; 0]);
    let mut sr = StreamReader::new(reader, DefaultDeframer::new());

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
