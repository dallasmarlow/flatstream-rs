use flatstream::*;

#[test]
fn deframers_produce_identical_payloads() {
    // Frame a payload
    let mut out = Vec::new();
    let payload = b"equivalence-check";
    DefaultFramer.frame_and_write(&mut out, payload).unwrap();

    // Read with DefaultDeframer
    let mut r_def = StreamReader::new(std::io::Cursor::new(&out), DefaultDeframer);
    let p_def = r_def.read_message().unwrap().unwrap().to_vec();

    // Read with SafeTakeDeframer
    let mut r_safe = StreamReader::new(std::io::Cursor::new(&out), SafeTakeDeframer);
    let p_safe = r_safe.read_message().unwrap().unwrap().to_vec();

    // Read with UnsafeDeframer
    let mut r_unsafe = StreamReader::new(std::io::Cursor::new(&out), UnsafeDeframer);
    let p_unsafe = r_unsafe.read_message().unwrap().unwrap().to_vec();

    assert_eq!(p_def, p_safe);
    assert_eq!(p_def, p_unsafe);
}
