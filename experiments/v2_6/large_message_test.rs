//! Demonstrating performance differences between simple and expert modes with large messages
//! This script showed that expert mode becomes more beneficial with large messages
//! and mixed message sizes.

use flatbuffers::FlatBufferBuilder;
use flatstream::*;
use std::io::Cursor;
use std::time::Instant;

#[derive(Clone)]
struct LargeMessage {
    data: Vec<u8>,
}

impl StreamSerialize for LargeMessage {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let data_vec = builder.create_vector(&self.data);
        builder.finish(data_vec, None);
        Ok(())
    }
}

#[derive(Clone)]
struct SmallMessage {
    id: u32,
}

impl StreamSerialize for SmallMessage {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let data = format!("msg_{}", self.id);
        let s = builder.create_string(&data);
        builder.finish(s, None);
        Ok(())
    }
}

fn main() {
    println!("=== Large Message Performance Test ===\n");
    
    // Test 1: Large messages only
    test_large_messages_only();
    
    // Test 2: Mixed message sizes
    test_mixed_message_sizes();
    
    // Test 3: Multiple builders in expert mode
    test_multiple_builders();
}

fn test_large_messages_only() {
    println!("1. Large Messages Only (10MB each):");
    
    let large_messages: Vec<LargeMessage> = (0..10)
        .map(|_| LargeMessage {
            data: vec![0xAB; 10 * 1024 * 1024], // 10MB of data
        })
        .collect();
    
    // Simple mode
    let start = Instant::now();
    {
        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
        
        for msg in &large_messages {
            writer.write(msg).unwrap();
        }
    }
    let simple_time = start.elapsed();
    
    // Expert mode
    let start = Instant::now();
    {
        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();
        
        for msg in &large_messages {
            builder.reset();
            msg.serialize(&mut builder).unwrap();
            writer.write_finished(&mut builder).unwrap();
        }
    }
    let expert_time = start.elapsed();
    
    println!("   Simple mode: {:?}", simple_time);
    println!("   Expert mode: {:?}", expert_time);
    println!("   Difference: {:.2}x\n", simple_time.as_secs_f64() / expert_time.as_secs_f64());
}

fn test_mixed_message_sizes() {
    println!("2. Mixed Message Sizes (alternating 10MB and 10 bytes):");
    
    let large_msg = LargeMessage {
        data: vec![0xAB; 10 * 1024 * 1024], // 10MB
    };
    let small_msg = SmallMessage { id: 42 };
    
    // Simple mode - single internal builder for all sizes
    let start = Instant::now();
    {
        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
        
        for i in 0..20 {
            if i % 2 == 0 {
                writer.write(&large_msg).unwrap();
            } else {
                writer.write(&small_msg).unwrap();
            }
        }
    }
    let simple_time = start.elapsed();
    println!("   Simple mode (one builder for all): {:?}", simple_time);
    
    // Expert mode - still using one builder (suboptimal)
    let start = Instant::now();
    {
        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();
        
        for i in 0..20 {
            if i % 2 == 0 {
                builder.reset();
                large_msg.serialize(&mut builder).unwrap();
                writer.write_finished(&mut builder).unwrap();
            } else {
                builder.reset();
                small_msg.serialize(&mut builder).unwrap();
                writer.write_finished(&mut builder).unwrap();
            }
        }
    }
    let expert_single_time = start.elapsed();
    println!("   Expert mode (one builder): {:?}", expert_single_time);
    
    // Memory impact
    println!("   Note: After serializing 10MB message, builder retains that capacity");
    println!("         even when serializing tiny messages!\n");
}

fn test_multiple_builders() {
    println!("3. Expert Mode with Multiple Builders (optimal for mixed sizes):");
    
    let large_msg = LargeMessage {
        data: vec![0xAB; 10 * 1024 * 1024], // 10MB
    };
    let small_msg = SmallMessage { id: 42 };
    
    let start = Instant::now();
    {
        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
        
        // Maintain separate builders for different message types/sizes
        let mut large_builder = FlatBufferBuilder::new();
        let mut small_builder = FlatBufferBuilder::new();
        
        for i in 0..20 {
            if i % 2 == 0 {
                large_builder.reset();
                large_msg.serialize(&mut large_builder).unwrap();
                writer.write_finished(&mut large_builder).unwrap();
            } else {
                small_builder.reset();
                small_msg.serialize(&mut small_builder).unwrap();
                writer.write_finished(&mut small_builder).unwrap();
            }
        }
    }
    let expert_multi_time = start.elapsed();
    
    println!("   Expert mode (separate builders): {:?}", expert_multi_time);
    println!("   This avoids memory waste by using right-sized builders");
    println!("\n   Key insight: Simple mode can't do this optimization!");
    println!("   With simple mode, you're stuck with one internal builder that");
    println!("   grows to accommodate your largest message and stays that size.");
} 