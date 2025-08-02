//! Overhead analysis to understand where the performance difference comes from
//! This script helped identify that the overhead is minimal and comes from
//! trait dispatch, not from allocations or copying.

use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::io::Cursor;
use std::time::Instant;

struct DummyMessage;

impl StreamSerialize for DummyMessage {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let s = builder.create_string("x");
        builder.finish(s, None);
        Ok(())
    }
}

fn main() {
    println!("=== Overhead Analysis ===\n");
    
    const ITERATIONS: u32 = 10_000;
    
    // Test 1: Just builder reset overhead
    let start = Instant::now();
    let mut builder = FlatBufferBuilder::new();
    for _ in 0..ITERATIONS {
        builder.reset();
        let s = builder.create_string("x");
        builder.finish(s, None);
        let _ = builder.finished_data();
    }
    let builder_only = start.elapsed();
    
    // Test 2: Simple mode overhead
    let start = Instant::now();
    let mut buffer = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
    let msg = DummyMessage;
    for _ in 0..ITERATIONS {
        writer.write(&msg).unwrap();
    }
    let simple_mode = start.elapsed();
    
    // Test 3: Expert mode overhead
    let start = Instant::now();
    let mut buffer = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
    let mut builder = FlatBufferBuilder::new();
    for _ in 0..ITERATIONS {
        builder.reset();
        msg.serialize(&mut builder).unwrap();
        writer.write_finished(&mut builder).unwrap();
    }
    let expert_mode = start.elapsed();
    
    // Test 4: Raw framing overhead (baseline)
    let start = Instant::now();
    let mut buffer = Vec::new();
    let framer = DefaultFramer;
    let mut cursor = Cursor::new(&mut buffer);
    for _ in 0..ITERATIONS {
        framer.frame_and_write(&mut cursor, b"test").unwrap();
    }
    let raw_framing = start.elapsed();
    
    println!("Results for {} iterations:", ITERATIONS);
    println!("  Builder operations only: {:?}", builder_only);
    println!("  Simple mode total: {:?}", simple_mode);
    println!("  Expert mode total: {:?}", expert_mode);
    println!("  Raw framing only: {:?}", raw_framing);
    println!();
    println!("Per-operation breakdown:");
    println!("  Builder reset+serialize: {:.2}ns", builder_only.as_nanos() as f64 / ITERATIONS as f64);
    println!("  Simple mode: {:.2}ns", simple_mode.as_nanos() as f64 / ITERATIONS as f64);
    println!("  Expert mode: {:.2}ns", expert_mode.as_nanos() as f64 / ITERATIONS as f64);
    println!("  Raw framing: {:.2}ns", raw_framing.as_nanos() as f64 / ITERATIONS as f64);
    println!();
    println!("Simple vs Expert overhead: {:.2}ns per operation", 
        (simple_mode.as_nanos() as f64 - expert_mode.as_nanos() as f64) / ITERATIONS as f64);
} 