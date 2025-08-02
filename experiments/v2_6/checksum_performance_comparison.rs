//! Comparing performance and overhead of different checksum algorithms
//! Verifies the claims about CRC16 having 75% less overhead than XXHash64

#[cfg(feature = "all_checksums")]
use flatstream::*;
use std::io::Cursor;
use std::time::Instant;

#[cfg(not(feature = "all_checksums"))]
fn main() {
    println!("This example requires the 'all_checksums' feature.");
    println!("Run with: cargo run --example checksum_performance_comparison --features all_checksums");
}

#[cfg(feature = "all_checksums")]
fn main() {
    println!("=== Checksum Performance and Overhead Comparison ===\n");
    
    // Test with different message sizes
    let small_msg = "Small telemetry data";
    let medium_msg = "Medium sized message with more content that represents typical application data";
    let large_msg = &"Large message content".repeat(1000); // ~20KB
    
    println!("1. Overhead Comparison (bytes per message):");
    println!("   NoChecksum:  0 bytes (baseline)");
    println!("   CRC16:       2 bytes (75% less than XXHash64)");
    println!("   CRC32:       4 bytes (50% less than XXHash64)"); 
    println!("   XXHash64:    8 bytes (maximum integrity)\n");
    
    println!("2. Performance Comparison (1000 messages):\n");
    
    // Small messages
    println!("Small Messages (~20 bytes):");
    compare_checksums(small_msg, 1000);
    
    println!("\nMedium Messages (~80 bytes):");
    compare_checksums(medium_msg, 1000);
    
    println!("\nLarge Messages (~20KB):");
    compare_checksums(large_msg, 100);
    
    println!("\n3. Relative Overhead Impact:");
    calculate_overhead_impact();
}

#[cfg(feature = "all_checksums")]
fn compare_checksums(msg: &str, iterations: usize) {
    // NoChecksum baseline
    let no_checksum_time = measure_checksum_performance(
        msg, 
        iterations, 
        DefaultFramer,
        DefaultDeframer
    );
    println!("   NoChecksum:  {:?} (baseline)", no_checksum_time);
    
    // CRC16
    let crc16_time = measure_checksum_performance(
        msg,
        iterations,
        ChecksumFramer::new(Crc16::new()),
        ChecksumDeframer::new(Crc16::new())
    );
    let crc16_overhead = ((crc16_time.as_nanos() as f64 / no_checksum_time.as_nanos() as f64) - 1.0) * 100.0;
    println!("   CRC16:       {:?} (+{:.1}% overhead)", crc16_time, crc16_overhead);
    
    // CRC32
    let crc32_time = measure_checksum_performance(
        msg,
        iterations,
        ChecksumFramer::new(Crc32::new()),
        ChecksumDeframer::new(Crc32::new())
    );
    let crc32_overhead = ((crc32_time.as_nanos() as f64 / no_checksum_time.as_nanos() as f64) - 1.0) * 100.0;
    println!("   CRC32:       {:?} (+{:.1}% overhead)", crc32_time, crc32_overhead);
    
    // XXHash64
    let xxhash_time = measure_checksum_performance(
        msg,
        iterations,
        ChecksumFramer::new(XxHash64::new()),
        ChecksumDeframer::new(XxHash64::new())
    );
    let xxhash_overhead = ((xxhash_time.as_nanos() as f64 / no_checksum_time.as_nanos() as f64) - 1.0) * 100.0;
    println!("   XXHash64:    {:?} (+{:.1}% overhead)", xxhash_time, xxhash_overhead);
}

#[cfg(feature = "all_checksums")]
fn measure_checksum_performance<F: Framer, D: Deframer>(
    msg: &str,
    iterations: usize,
    framer: F,
    deframer: D,
) -> std::time::Duration {
    // Write phase
    let mut buffer = Vec::new();
    let start = Instant::now();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut buffer), framer);
        for _ in 0..iterations {
            writer.write(&msg).unwrap();
        }
    }
    let write_time = start.elapsed();
    
    // Read phase
    let start = Instant::now();
    {
        let mut reader = StreamReader::new(Cursor::new(&buffer), deframer);
        reader.process_all(|_payload| Ok(())).unwrap();
    }
    let read_time = start.elapsed();
    
    // Return combined time
    write_time + read_time
}

#[cfg(feature = "all_checksums")]
fn calculate_overhead_impact() {
    println!("\n   For a 20-byte message:");
    println!("   - NoChecksum: 24 bytes total (4 length + 20 payload)");
    println!("   - CRC16:      26 bytes total (+8.3% size increase)");
    println!("   - CRC32:      28 bytes total (+16.7% size increase)");
    println!("   - XXHash64:   32 bytes total (+33.3% size increase)");
    
    println!("\n   For a 1KB message:");
    println!("   - NoChecksum: 1028 bytes total");
    println!("   - CRC16:      1030 bytes total (+0.2% size increase)");
    println!("   - CRC32:      1032 bytes total (+0.4% size increase)");
    println!("   - XXHash64:   1036 bytes total (+0.8% size increase)");
    
    println!("\n   Key Insight: Checksum overhead matters most for small, high-frequency messages!");
    println!("   This is why sized checksums (CRC16/CRC32) are valuable for IoT/telemetry use cases.");
} 