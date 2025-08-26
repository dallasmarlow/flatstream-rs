// Example purpose: Demonstrates zero-allocation read patterns using process_all() and
// messages() along with BufReader. Highlights payload lifetime rules (valid until next read).
use flatstream::{
    DefaultDeframer, DefaultFramer, Result, SafeTakeDeframer, StreamReader, StreamWriter,
    UnsafeDeframer,
};
use std::io::Cursor;
use std::time::Instant;

fn main() -> Result<()> {
    println!("=== Deframer Performance Example ===\n");

    // 1. Prepare a consistent set of test data
    let mut buffer = Vec::new();
    let mut writer = StreamWriter::new(Cursor::new(&mut buffer), DefaultFramer);
    for i in 0..10_000 {
        let msg = format!("message number {i}");
        writer.write(&msg)?;
    }
    println!("Created test data with 10,000 messages.\n");

    // 2. Demonstrate and time each deframer
    println!("Running performance comparison...\n");

    // --- DefaultDeframer ---
    let start = Instant::now();
    let mut reader_default = StreamReader::new(Cursor::new(&buffer), DefaultDeframer);
    reader_default.process_all(|_| Ok(()))?;
    let duration_default = start.elapsed();
    println!("DefaultDeframer (Safe, General Purpose):  {duration_default:?}");

    // --- SafeTakeDeframer ---
    let start = Instant::now();
    let mut reader_safe_take = StreamReader::new(Cursor::new(&buffer), SafeTakeDeframer);
    reader_safe_take.process_all(|_| Ok(()))?;
    let duration_safe_take = start.elapsed();
    println!("SafeTakeDeframer (Safe, Alternative):     {duration_safe_take:?}");

    // --- UnsafeDeframer ---
    let start = Instant::now();
    let mut reader_unsafe = StreamReader::new(Cursor::new(&buffer), UnsafeDeframer);
    reader_unsafe.process_all(|_| Ok(()))?;
    let duration_unsafe = start.elapsed();
    println!("UnsafeDeframer (Fastest, Trusted Input):  {duration_unsafe:?}");

    println!("\n=== Guidance ===");
    println!("- Use DefaultDeframer for most cases. It's safe and performs well.");
    println!("- Use SafeTakeDeframer if you have a custom `Read` implementation where `take()` is optimized.");
    println!("- Use UnsafeDeframer only in performance-critical paths where you trust the data source and need to avoid buffer zeroing at all costs.");

    Ok(())
}
