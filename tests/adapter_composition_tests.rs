use flatstream::framing::{DeframerExt, FramerExt};
use flatstream::*;
use std::cell::Cell;

#[test]
fn bounded_observed_framer_works() {
    let seen = Cell::new(false);
    let framer = DefaultFramer.bounded(128).observed(|p| {
        seen.set(true);
        assert!(!p.is_empty());
    });

    // Valid payload
    let mut ok = Vec::new();
    framer.frame_and_write(&mut ok, b"valid").unwrap();
    assert!(seen.get());

    // Invalid payload
    let mut sink = Vec::new();
    let result = framer.frame_and_write(&mut sink, &[0u8; 200]);
    assert!(matches!(result, Err(Error::InvalidFrame { .. })));
}

#[test]
fn bounded_observed_deframer_works() {
    let seen = Cell::new(false);
    let deframer = DefaultDeframer.bounded(128).observed(|p| {
        seen.set(true);
        assert!(p.len() <= 128);
    });

    // Frame a valid payload
    let mut out = Vec::new();
    DefaultFramer.frame_and_write(&mut out, b"valid").unwrap();

    // Read it back
    let mut buf = Vec::new();
    let mut cur = std::io::Cursor::new(&out);
    deframer
        .read_and_deframe(&mut cur, &mut buf)
        .unwrap()
        .unwrap();
    assert!(seen.get());
}
