//! Performance comparison between simple and expert modes
//! This script was used to understand the actual performance differences
//! between the two writing modes in v2.6.

use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::io::Cursor;
use std::time::Instant;

#[derive(Clone)]
struct TestMessage {
    id: u32,
    data: String,
}

impl StreamSerialize for TestMessage {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let data_offset = builder.create_string(&self.data);
        builder.finish(data_offset, None);
        Ok(())
    }
}

fn main() {
    println!("=== Simple vs Expert Mode Performance Comparison ===\n");
    
    let messages: Vec<TestMessage> = (0..1000)
        .map(|i| TestMessage {
            id: i,
            data: format!("Test message number {}", i),
        })
        .collect();
    
    // Warm up
    run_simple_mode(&messages);
    run_expert_mode(&messages);
    
    // Actual measurements
    println!("Running performance tests (1000 messages)...\n");
    
    let simple_time = run_simple_mode(&messages);
    let expert_time = run_expert_mode(&messages);
    
    println!("Results:");
    println!("  Simple mode: {:?}", simple_time);
    println!("  Expert mode: {:?}", expert_time);
    println!("  Difference: {:.2}%", ((simple_time.as_secs_f64() / expert_time.as_secs_f64()) - 1.0) * 100.0);
    println!("  Per message overhead: {:.2}ns", (simple_time.as_nanos() as f64 - expert_time.as_nanos() as f64) / 1000.0);
}

fn run_simple_mode(messages: &[TestMessage]) -> std::time::Duration {
    let start = Instant::now();
    
    let mut buffer = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
    
    for msg in messages {
        writer.write(msg).unwrap();
    }
    
    start.elapsed()
}

fn run_expert_mode(messages: &[TestMessage]) -> std::time::Duration {
    let start = Instant::now();
    
    let mut buffer = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
    let mut builder = FlatBufferBuilder::new();
    
    for msg in messages {
        builder.reset();
        msg.serialize(&mut builder).unwrap();
        writer.write_finished(&mut builder).unwrap();
    }
    
    start.elapsed()
} 