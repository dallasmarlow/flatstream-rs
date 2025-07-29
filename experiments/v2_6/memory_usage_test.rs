//! Demonstrating memory usage differences between simple and expert modes
//! This script showed how simple mode can lead to memory bloat when dealing
//! with mixed message sizes.

use flatbuffers::FlatBufferBuilder;
use flatstream_rs::*;
use std::io::Cursor;

struct TinyMessage {
    id: u32,
}

impl StreamSerialize for TinyMessage {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let s = builder.create_string(&format!("{}", self.id));
        builder.finish(s, None);
        Ok(())
    }
}

struct HugeMessage {
    data: Vec<u8>,
}

impl StreamSerialize for HugeMessage {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        let vec = builder.create_vector(&self.data);
        builder.finish(vec, None);
        Ok(())
    }
}

fn main() {
    println!("=== Memory Usage Analysis ===\n");
    
    // Scenario: Write one huge message, then many tiny messages
    println!("Scenario: 1 huge message (50MB), then 1000 tiny messages (10 bytes each)\n");
    
    // Simple mode - problematic memory usage
    println!("1. Simple Mode (cannot optimize memory):");
    {
        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
        
        // Write one huge message
        let huge = HugeMessage { data: vec![0xFF; 50 * 1024 * 1024] };
        writer.write(&huge).unwrap();
        println!("   After huge message: Internal builder has 50MB+ capacity");
        
        // Now write many tiny messages
        for i in 0..1000 {
            let tiny = TinyMessage { id: i };
            writer.write(&tiny).unwrap();
        }
        println!("   After 1000 tiny messages: Builder STILL has 50MB+ capacity!");
        println!("   Memory waste: ~50MB held unnecessarily\n");
    }
    
    // Expert mode - can optimize memory usage
    println!("2. Expert Mode (can use separate builders):");
    {
        let mut buffer = Vec::new();
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
        
        // Use a temporary builder for the huge message
        {
            let mut huge_builder = FlatBufferBuilder::new();
            let huge = HugeMessage { data: vec![0xFF; 50 * 1024 * 1024] };
            huge_builder.reset();
            huge.serialize(&mut huge_builder).unwrap();
            writer.write_finished(&mut huge_builder).unwrap();
            // huge_builder is dropped here, freeing the 50MB
        }
        println!("   After huge message: Temporary builder dropped, memory freed");
        
        // Use a small builder for tiny messages
        let mut tiny_builder = FlatBufferBuilder::new();
        for i in 0..1000 {
            let tiny = TinyMessage { id: i };
            tiny_builder.reset();
            tiny.serialize(&mut tiny_builder).unwrap();
            writer.write_finished(&mut tiny_builder).unwrap();
        }
        println!("   After 1000 tiny messages: Only using ~1KB for tiny_builder");
        println!("   Memory efficient: Large allocation was freed after use\n");
    }
    
    println!("3. Real-world Example: Multi-Type Message System");
    println!("   Imagine a system handling:");
    println!("   - Control messages (tiny, frequent)");
    println!("   - Telemetry batches (medium, periodic)"); 
    println!("   - File transfers (huge, rare)");
    println!("\n   With Simple Mode:");
    println!("   - One file transfer permanently bloats the builder");
    println!("   - All subsequent tiny messages waste memory");
    println!("\n   With Expert Mode:");
    println!("   - Maintain 3 builders: control_builder, telemetry_builder, file_builder");
    println!("   - Each builder sized appropriately for its use case");
    println!("   - Can even drop/recreate file_builder as needed");
} 