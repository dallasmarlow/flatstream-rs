// Example purpose: The reference end-to-end workload — a telemetry capture agent.
// Expert-mode writes with a reused builder, real FlatBuffers payloads (a vector of
// f64 channels — no generated code required), zero-copy *typed* reads that compute
// real statistics, and manual iteration with early exit.
//
// Every claim this example makes is asserted: the data is deterministic, so the
// exact alert counts and channel averages are derivable by hand (the arithmetic is
// in the comments) and the example fails loudly if the library or the reasoning is
// wrong. Run it to prove the behavior, not to admire the printouts.

use flatbuffers::FlatBufferBuilder;
use flatstream::{
    DefaultDeframer, DefaultFramer, Error, Result, StreamDeserialize, StreamReader, StreamWriter,
};
use std::io::Cursor;

// Each frame's payload is a FlatBuffers vector of f64 channels, addressed by
// index. This keeps the example free of flatc-generated code while still being
// a real FlatBuffer read zero-copy via typed access — for named-field schemas,
// see `typed_reading_flatc_example.rs`.
const IDX_TIMESTAMP_S: usize = 0;
const IDX_SPEED_KPH: usize = 1;
const IDX_RPM: usize = 2;
const IDX_TEMP_C: usize = 3;
const IDX_BATTERY_PCT: usize = 4;

const EVENT_COUNT: usize = 2_000;

/// Deterministic tick generator (100 Hz). The formulas are chosen so the alert
/// counts below are exact:
///   speed  = (i % 200) * 0.5      → > 80 ⇔ i%200 ≥ 161 → 39 ticks/cycle × 10 cycles = 390
///   temp   = 20 + (i % 40)        → > 50 ⇔ i%40  ≥ 31 →  9 ticks/cycle × 50 cycles = 450
///   battery= 100 − i·0.045        → < 20 ⇔ i ≥ 1778   → 2000 − 1778               = 222
fn channels_at(i: usize) -> [f64; 5] {
    [
        i as f64 * 0.01,
        (i % 200) as f64 * 0.5,
        800.0 + i as f64,
        20.0 + (i % 40) as f64,
        100.0 - i as f64 * 0.045,
    ]
}

/// Typed zero-copy access: the root is a FlatBuffers vector borrowed straight
/// from the reader's buffer — no deserialization, no allocation per frame.
struct TelemetryFrame;

impl<'a> StreamDeserialize<'a> for TelemetryFrame {
    type Root = flatbuffers::Vector<'a, f64>;
    fn from_payload(payload: &'a [u8]) -> Result<Self::Root> {
        flatbuffers::root::<flatbuffers::Vector<'a, f64>>(payload).map_err(Error::from)
    }
}

fn main() -> Result<()> {
    println!("=== Telemetry Agent (reference workload) ===\n");

    // --- Capture: expert mode, one builder reused across all frames -----------
    let mut journal = Vec::new();
    {
        let mut writer = StreamWriter::new(Cursor::new(&mut journal), DefaultFramer);
        let mut builder = FlatBufferBuilder::new();
        for i in 0..EVENT_COUNT {
            builder.reset(); // reuse the builder's allocation every tick
            let v = builder.create_vector(&channels_at(i));
            builder.finish(v, None);
            writer.write_finished(&mut builder)?;
        }
        writer.flush()?;
    }
    println!(
        "captured {EVENT_COUNT} frames, {} bytes on the wire",
        journal.len()
    );

    // --- Analysis: typed zero-copy scan over the whole stream -----------------
    let mut reader = StreamReader::new(Cursor::new(&journal), DefaultDeframer::new());
    let (mut frames, mut speed_sum, mut temp_sum, mut rpm_sum) = (0usize, 0.0f64, 0.0f64, 0.0f64);
    let (mut speed_alerts, mut temp_alerts, mut battery_alerts) = (0u32, 0u32, 0u32);

    reader.process_typed::<TelemetryFrame, _>(|channels| {
        frames += 1;
        let speed = channels.get(IDX_SPEED_KPH);
        let temp = channels.get(IDX_TEMP_C);
        speed_sum += speed;
        temp_sum += temp;
        rpm_sum += channels.get(IDX_RPM);
        if speed > 80.0 {
            speed_alerts += 1;
        }
        if temp > 50.0 {
            temp_alerts += 1;
        }
        if channels.get(IDX_BATTERY_PCT) < 20.0 {
            battery_alerts += 1;
        }
        Ok(())
    })?;

    let avg_speed = speed_sum / frames as f64;
    let avg_temp = temp_sum / frames as f64;
    let avg_rpm = rpm_sum / frames as f64;
    println!("frames processed: {frames}");
    println!("avg speed: {avg_speed:.2} km/h   avg rpm: {avg_rpm:.1}   avg temp: {avg_temp:.1} °C");
    println!("alerts — speed: {speed_alerts}, temp: {temp_alerts}, battery: {battery_alerts}");

    // Prove the run against the hand-derived expectations (see channels_at).
    assert_eq!(frames, EVENT_COUNT);
    assert_eq!(speed_alerts, 390);
    assert_eq!(temp_alerts, 450);
    assert_eq!(battery_alerts, 222);
    // avg(speed) = 0.5 · mean(0..=199) = 49.75; avg(temp) = 20 + mean(0..=39) = 39.5;
    // avg(rpm) = 800 + mean(0..=1999) = 1799.5
    assert!((avg_speed - 49.75).abs() < 1e-9);
    assert!((avg_temp - 39.5).abs() < 1e-9);
    assert!((avg_rpm - 1799.5).abs() < 1e-9);

    // --- Manual iteration with early exit: find the first speed alert ---------
    // `messages()` yields raw payloads under caller control — the pattern for
    // scans that stop early instead of processing the whole stream.
    let mut reader = StreamReader::new(Cursor::new(&journal), DefaultDeframer::new());
    let mut messages = reader.messages();
    let mut examined = 0usize;
    let first_alert_ts = loop {
        match messages.next()? {
            Some(payload) => {
                examined += 1;
                let channels = TelemetryFrame::from_payload(payload)?;
                if channels.get(IDX_SPEED_KPH) > 80.0 {
                    break Some(channels.get(IDX_TIMESTAMP_S));
                }
            }
            None => break None,
        }
    };
    // First i with (i % 200)·0.5 > 80 is 161 → timestamp 1.61 s, 162 frames read.
    let ts = first_alert_ts.expect("an alert exists in the stream");
    println!("first speed alert at t={ts:.2}s after examining {examined} frames (early exit)");
    assert_eq!(examined, 162);
    assert!((ts - 1.61).abs() < 1e-9);

    println!("\nall assertions passed — behavior proven, not assumed ✓");
    Ok(())
}
