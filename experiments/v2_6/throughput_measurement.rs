//! Measuring actual throughput to verify performance claims in the documentation
//! This demonstrates the library's capability for high-frequency telemetry

use flatbuffers::FlatBufferBuilder;
use flatstream_rs::*;
use std::io::Cursor;
use std::time::{Duration, Instant};

#[derive(Clone)]
struct TelemetryEvent {
    timestamp: u64,
    sensor_id: u32,
    value: f64,
}

impl StreamSerialize for TelemetryEvent {
    fn serialize<A: flatbuffers::Allocator>(
        &self,
        builder: &mut FlatBufferBuilder<A>,
    ) -> Result<()> {
        // Simulate a realistic telemetry message
        let sensor_name = format!("sensor_{}", self.sensor_id);
        let offset = builder.create_string(&sensor_name);
        builder.finish(offset, None);
        Ok(())
    }
}

fn main() {
    println!("=== Throughput Measurement ===\n");
    
    println!("1. Write Throughput (Simple Mode):");
    measure_write_throughput_simple();
    
    println!("\n2. Write Throughput (Expert Mode):");
    measure_write_throughput_expert();
    
    println!("\n3. Read Throughput:");
    measure_read_throughput();
    
    println!("\n4. End-to-End Throughput:");
    measure_end_to_end();
    
    println!("\n5. High-Frequency Telemetry Simulation:");
    simulate_telemetry_agent();
}

fn measure_write_throughput_simple() {
    let mut buffer = Vec::with_capacity(10_000_000); // 10MB
    let events = generate_events(10_000);
    
    let start = Instant::now();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
        for event in &events {
            writer.write(event).unwrap();
        }
    }
    let elapsed = start.elapsed();
    
    calculate_and_print_throughput("Simple mode write", events.len(), buffer.len(), elapsed);
}

fn measure_write_throughput_expert() {
    let mut buffer = Vec::with_capacity(10_000_000); // 10MB
    let events = generate_events(10_000);
    
    let start = Instant::now();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();
        
        for event in &events {
            builder.reset();
            event.serialize(&mut builder).unwrap();
            writer.write_finished(&mut builder).unwrap();
        }
    }
    let elapsed = start.elapsed();
    
    calculate_and_print_throughput("Expert mode write", events.len(), buffer.len(), elapsed);
}

fn measure_read_throughput() {
    // First, create test data
    let mut buffer = Vec::new();
    let events = generate_events(10_000);
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
        for event in &events {
            writer.write(event).unwrap();
        }
    }
    
    // Measure read throughput
    let start = Instant::now();
    {
        let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
        let mut count = 0;
        reader.process_all(|_payload| {
            count += 1;
            Ok(())
        }).unwrap();
        assert_eq!(count, events.len());
    }
    let elapsed = start.elapsed();
    
    calculate_and_print_throughput("Read", events.len(), buffer.len(), elapsed);
}

fn measure_end_to_end() {
    let events = generate_events(1_000);
    
    let start = Instant::now();
    
    // Write phase
    let mut buffer = Vec::new();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();
        
        for event in &events {
            builder.reset();
            event.serialize(&mut builder).unwrap();
            writer.write_finished(&mut builder).unwrap();
        }
    }
    
    // Read phase
    {
        let mut reader = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
        reader.process_all(|_payload| Ok(())).unwrap();
    }
    
    let elapsed = start.elapsed();
    calculate_and_print_throughput("End-to-end", events.len(), buffer.len(), elapsed);
}

fn simulate_telemetry_agent() {
    println!("   Simulating 1 second of telemetry capture...");
    
    let mut total_events = 0;
    let mut total_bytes = 0;
    let duration = Duration::from_secs(1);
    let start = Instant::now();
    
    let mut buffer = Vec::with_capacity(100_000_000); // 100MB
    let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
    let mut builder = FlatBufferBuilder::new();
    
    // Generate events at ~100k Hz
    let mut event_count = 0;
    while start.elapsed() < duration {
        let event = TelemetryEvent {
            timestamp: start.elapsed().as_micros() as u64,
            sensor_id: (event_count % 100) as u32,
            value: (event_count as f64) * 0.1,
        };
        
        builder.reset();
        event.serialize(&mut builder).unwrap();
        writer.write_finished(&mut builder).unwrap();
        
        event_count += 1;
        
        // Batch check time to avoid overhead
        if event_count % 1000 == 0 && start.elapsed() >= duration {
            break;
        }
    }
    
    total_events = event_count;
    total_bytes = buffer.len();
    
    let actual_duration = start.elapsed();
    
    println!("   Results:");
    println!("   - Events captured: {}", total_events);
    println!("   - Data written: {} MB", total_bytes as f64 / 1_000_000.0);
    println!("   - Duration: {:?}", actual_duration);
    println!("   - Event rate: {} events/sec", 
        (total_events as f64 / actual_duration.as_secs_f64()) as u64);
    println!("   - Throughput: {:.1} MB/sec", 
        (total_bytes as f64 / 1_000_000.0) / actual_duration.as_secs_f64());
    
    // Compare to documented claims
    println!("\n   Comparison to documentation:");
    if total_events > 50_000 {
        println!("   ✓ Achieved >50k messages/sec as claimed");
    } else {
        println!("   ✗ Below claimed 50k messages/sec");
    }
}

fn generate_events(count: usize) -> Vec<TelemetryEvent> {
    (0..count)
        .map(|i| TelemetryEvent {
            timestamp: 1000000 + i as u64,
            sensor_id: (i % 100) as u32,
            value: (i as f64) * 0.1,
        })
        .collect()
}

fn calculate_and_print_throughput(operation: &str, messages: usize, bytes: usize, elapsed: Duration) {
    let messages_per_sec = messages as f64 / elapsed.as_secs_f64();
    let mb_per_sec = (bytes as f64 / 1_000_000.0) / elapsed.as_secs_f64();
    let ns_per_message = elapsed.as_nanos() as f64 / messages as f64;
    
    println!("   {} performance:", operation);
    println!("   - Messages: {}", messages);
    println!("   - Total size: {:.2} MB", bytes as f64 / 1_000_000.0);
    println!("   - Time: {:?}", elapsed);
    println!("   - Throughput: {:.0} messages/sec", messages_per_sec);
    println!("   - Throughput: {:.1} MB/sec", mb_per_sec);
    println!("   - Latency: {:.0} ns/message", ns_per_message);
} 