use flatstream::framing::DefaultFramer;
use flatstream::*;

#[test]
fn defaultframer_layout() {
    let payload = b"abc";
    let mut out = Vec::new();
    DefaultFramer.frame_and_write(&mut out, payload).unwrap();
    assert_eq!(out.len(), 4 + payload.len());
    let len = u32::from_le_bytes([out[0], out[1], out[2], out[3]]) as usize;
    assert_eq!(len, payload.len());
    assert_eq!(&out[4..], payload);
}

#[cfg(feature = "xxhash")]
#[test]
fn checksumframer_layout_xxhash64() {
    use flatstream::framing::ChecksumFramer;
    let payload = b"abc";
    let mut out = Vec::new();
    let framer = ChecksumFramer::new(XxHash64::new());
    framer.frame_and_write(&mut out, payload).unwrap();
    assert_eq!(out.len(), 4 + 8 + payload.len());
    let len = u32::from_le_bytes([out[0], out[1], out[2], out[3]]) as usize;
    assert_eq!(len, payload.len());
    let _cksum = u64::from_le_bytes(out[4..12].try_into().unwrap());
    assert_eq!(&out[12..], payload);
}
