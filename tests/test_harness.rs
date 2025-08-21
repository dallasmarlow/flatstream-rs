use flatstream::{Deframer, Framer, StreamReader, StreamWriter};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

pub struct TestHarness {
    _temp_file: NamedTempFile,
    path: PathBuf,
}

impl TestHarness {
    pub fn new() -> Self {
        let temp_file = NamedTempFile::new().unwrap();
        let path = temp_file.path().to_path_buf();
        Self {
            _temp_file: temp_file,
            path,
        }
    }

    pub fn writer<F: Framer>(&self, framer: F) -> StreamWriter<BufWriter<File>, F> {
        let file = File::create(&self.path).unwrap();
        StreamWriter::new(BufWriter::new(file), framer)
    }

    pub fn reader<D: Deframer>(&self, deframer: D) -> StreamReader<BufReader<File>, D> {
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
