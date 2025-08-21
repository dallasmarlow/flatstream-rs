use flatbuffers::FlatBufferBuilder;
use flatstream::*;
mod test_harness;
use test_harness::TestHarness;

#[test]
fn table_driven_basic_cycles() {
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
        // read
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
    }
}
