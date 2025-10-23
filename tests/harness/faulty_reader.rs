use std::io::{Read, Result};

pub struct FaultyReader<R: Read> {
    inner: R,
    mode: FaultMode,
    counter: usize,
}

pub enum FaultMode {
    OneByteChunks,
    InterruptedEvery(usize),
    PrematureEofAt(usize),
}

impl<R: Read> FaultyReader<R> {
    pub fn new(inner: R, mode: FaultMode) -> Self {
        Self {
            inner,
            mode,
            counter: 0,
        }
    }
}

impl<R: Read> Read for FaultyReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.counter += 1;
        match self.mode {
            FaultMode::OneByteChunks => {
                let mut tmp = [0u8; 1];
                match self.inner.read(&mut tmp) {
                    Ok(0) => Ok(0),
                    Ok(1) => {
                        buf[0] = tmp[0];
                        Ok(1)
                    }
                    Ok(_) => unreachable!(),
                    Err(e) => Err(e),
                }
            }
            FaultMode::InterruptedEvery(n) if n != 0 && self.counter.is_multiple_of(n) => {
                Err(std::io::Error::from(std::io::ErrorKind::Interrupted))
            }
            FaultMode::PrematureEofAt(n) if self.counter >= n => Ok(0),
            _ => self.inner.read(buf),
        }
    }
}
