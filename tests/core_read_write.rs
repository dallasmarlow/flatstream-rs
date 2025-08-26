use flatbuffers::FlatBufferBuilder;
use flatstream::*;
mod test_harness;
use test_harness::TestHarness;

#[test]
fn table_driven_basic_cycles() {
    // Purpose: Validate write+read cycles over a variety of message counts/sizes using
    // both the high-performance processor API (process_all) and the expert iterator (messages()).
    // Ensures correctness (counts match, non-empty payloads for non-empty cases).
    let mut h = TestHarness::new();
    let cases: &[(&str, Vec<usize>)] = &[
        ("empty", vec![]),
        ("one", vec![8]),
        ("few", vec![4, 16, 32]),
        ("many_small", vec![8; 100]),
    ];

    for (_name, sizes) in cases.iter() {
        let msgs = h.gen_mixed_messages(sizes);
        // write
        {
            let mut w = h.writer(DefaultFramer);
            let mut b = FlatBufferBuilder::new();
            for m in &msgs {
                b.reset();
                let s = b.create_string(m);
                b.finish(s, None);
                w.write_finished(&mut b).unwrap();
            }
            w.flush().unwrap();
        }
        // read via process_all
        {
            let mut r = h.reader(DefaultDeframer);
            let mut count = 0;
            r.process_all(|p| {
                assert!(!p.is_empty() || msgs.is_empty());
                count += 1;
                Ok(())
            })
            .unwrap();
            assert_eq!(count, msgs.len());
        }

        // read via messages() expert API
        {
            let mut r = h.reader(DefaultDeframer);
            let mut count = 0usize;
            let mut it = r.messages();
            while let Some(p) = it.next().unwrap() {
                assert!(!p.is_empty() || msgs.is_empty());
                count += 1;
            }
            assert_eq!(count, msgs.len());
        }
    }
}
