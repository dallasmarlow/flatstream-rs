use flatstream::{Deframer, Framer, StreamReader, StreamWriter};
use rand::{rngs::StdRng, Rng, SeedableRng};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

pub struct TestHarness {
    _temp_file: NamedTempFile,
    path: PathBuf,
    rng: StdRng,
}

impl TestHarness {
    pub fn new() -> Self {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_path_buf();
        let rng = StdRng::seed_from_u64(0x005E_ED42);
        Self {
            _temp_file: temp_file,
            path,
            rng,
        }
    }

    pub fn writer<F: Framer>(&self, framer: F) -> StreamWriter<'_, BufWriter<File>, F> {
        // Purpose: Provide a buffered writer bound to a temp file for end-to-end tests.
        let file = File::create(&self.path).unwrap();
        StreamWriter::new(BufWriter::new(file), framer)
    }

    pub fn reader<D: Deframer>(&self, deframer: D) -> StreamReader<BufReader<File>, D> {
        // Purpose: Provide a buffered reader bound to the same temp file.
        let file = File::open(&self.path).unwrap();
        StreamReader::new(BufReader::new(file), deframer)
    }

    #[allow(dead_code)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[allow(dead_code)]
    pub fn corrupt_last_byte(&self) {
        let mut data = fs::read(&self.path).unwrap();
        if !data.is_empty() {
            let last = data.len() - 1;
            data[last] ^= 1;
        }
        fs::write(&self.path, data).unwrap();
    }

    #[allow(dead_code)]
    pub fn truncate_last_bytes(&self, n: usize) {
        let data = fs::read(&self.path).unwrap();
        let new_len = data.len().saturating_sub(n);
        fs::write(&self.path, &data[..new_len]).unwrap();
    }
}

impl Default for TestHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl TestHarness {
    #[allow(dead_code)]
    pub fn gen_string(&mut self, len: usize) -> String {
        // Purpose: Generate a pseudo-random ASCII string of the requested length.
        let mut s = String::with_capacity(len);
        for _ in 0..len {
            let c = (b'a' + (self.rng.gen::<u8>() % 26)) as char;
            s.push(c);
        }
        s
    }

    #[allow(dead_code)]
    pub fn gen_mixed_messages(&mut self, sizes: &[usize]) -> Vec<String> {
        // Purpose: Generate a vector of random strings matching requested sizes.
        sizes.iter().map(|&n| self.gen_string(n)).collect()
    }
}

// A reusable macro for defining standard framer/deframer write-read cycle tests.
// Placed here to centralize common test patterns.
#[allow(unused_macros)]
macro_rules! test_framer_deframer_pair {
    ($test_name:ident, $framer:expr, $deframer:expr, $messages:expr) => {
        #[test]
        fn $test_name() {
            use flatbuffers::FlatBufferBuilder;
            use flatstream::*;

            let harness = TestHarness::new();

            // Write phase: frame and persist messages using the provided framer
            {
                let mut stream_writer = harness.writer($framer);
                let mut builder = FlatBufferBuilder::new();
                for msg in $messages {
                    builder.reset();
                    let data = builder.create_string(msg);
                    builder.finish(data, None);
                    stream_writer.write_finished(&mut builder).unwrap();
                }
                stream_writer.flush().unwrap();
            }

            // Read phase: deframe and count messages using the provided deframer
            {
                let mut stream_reader = harness.reader($deframer);
                let mut count = 0usize;
                stream_reader
                    .process_all(|payload| {
                        assert!(!payload.is_empty() || $messages.is_empty());
                        count += 1;
                        Ok(())
                    })
                    .unwrap();
                assert_eq!(count, $messages.len());
            }
        }
    };
}
