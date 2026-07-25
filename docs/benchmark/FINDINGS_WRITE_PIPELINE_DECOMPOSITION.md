# Findings: what fraction of an end-to-end write is flatstream?

**Author:** contributor (A1, `CONTRIBUTING.md` §6)
**Date:** 2026-07-24
**Status:** complete — isolated raw output committed at
`docs/benchmark/raw/a1_write_pipeline.txt`

## Hypothesis

A consumer (the terminal scrollback journal of `ONBOARDING.md` §7) observed a
large end-to-end write throughput drop and attributed it to **their own**
serialization and bookkeeping rather than to flatstream. `CONTRIBUTING.md` §1
forbids publishing that attribution until it is measured.

The hypothesis under test, stated so it can fail: **in a realistic journaling
write, flatstream's framing is a small single-digit share of per-record cost,
dominated by the application's own harvest/serialization and by the OS write
path.** If framing came out above ~20% of a non-durable record, the consumer's
attribution would be wrong and the optimization effort would belong in
`src/framing.rs`.

A secondary hypothesis, added because it is the first thing a journal author
asks next: **at the measured cadence of one `sync_data()` per 1000-record batch,
durability dwarfs everything else**. This experiment does not generalize that
share to every possible sync cadence or storage device.

## Methodology

### Environment

- rustc: `rustc 1.97.1 (8bab26f4f 2026-07-14) (Homebrew)` — the declared MSRV
- OS / arch: macOS 26.5.2 / arm64
- CPU: Apple M4, 10 cores, 32 GiB
- Relevant dependency versions: `flatstream` 0.2.8, `flatbuffers` 25.12.19,
  `crc32fast` 1.5.0 (from `Cargo.lock`)
- Tool: Criterion 0.5, default sampling
- Feature flags: `--features crc32`
- Baseline compared against: N/A — this is an absolute decomposition, not an
  A/B. Each rung is compared against the rung below it, within one run.

**Instrument choice.** Wall clock only. Per `CONTRIBUTING.md` §4 this is the
right instrument for throughput *shares* and the wrong one for sub-nanosecond
deltas — see Findings §F3, which is precisely why backlog item A2 exists.

### Steps

```bash
# Isolated: nothing else running on the machine (see Threats §T1).
scripts/bench_isolated.sh a1_write_pipeline \
  write_pipeline_decomposition Decomposition -- --features crc32 --locked
```

`Throughput::Elements(1000)` is set, so every Criterion figure below is read
directly as **nanoseconds per record**.

#### Why a cumulative ladder rather than seven isolated micro-benches

Seven independently-timed stages do not sum to the pipeline: cache residency,
inlining decisions, and register pressure all change once the stages run
together, and the residual gets silently attributed to whichever stage the
author is least suspicious of. Instead each rung here is **the previous rung
plus exactly one layer**, over an identical record stream, so a layer's cost is
the delta between two adjacent rungs measured *in situ*.

| Rung | Adds | Whose cost |
|---|---|---|
| `s1_harvest` | pull a pty chunk, stamp sequence + `Instant::elapsed()` | application |
| `s2_build` | + build the `TerminalChunk` FlatBuffer into a reused builder | application (FlatBuffers) |
| `s3_frame` | + `DefaultFramer` into a reused `Vec` | **flatstream** |
| `s4_frame_crc32` | + CRC-32 over the payload (`ChecksumFramer`) | **flatstream** |
| `s5_buffered_file` | + sink becomes `BufWriter<File>` rather than `Vec` | OS / libstd |
| `s6_index` | + external offset index built from `FrameReceipt` | application |
| `s7_fsync` | + `flush()` + `sync_data()` once per 1000-record batch | durability |

The workload is the `ONBOARDING.md` §7 terminal-journaling profile: a
`TerminalChunk` table (`sequence`, `monotonic_timestamp`, `channel`,
`data:[ubyte]`) framed with `ChecksumFramer::new(Crc32::new())`. The timestamp
is a real `Instant::elapsed()`, not a constant — a journal that cannot order its
records is not a journal, so that cost belongs to the workload.

#### Guards against the ladder lying

Two independent cross-checks, run in the same process:

- `x_crc32_only` — CRC-32 timed in isolation over the same payloads. Must agree
  with `s4 − s3`.
- `x_frame_sink` — framing into `io::sink()`, which accepts and discards without
  a copy. Separates framing's *call* overhead from the payload copy that the
  `Vec` rung includes.

The scratch file is created once and rewound per iteration, so `s5`–`s7` measure
steady-state append into page cache, not file creation.

## Findings

All figures below are Criterion medians from isolated runs, in nanoseconds per
record. "Δ" is the cost of the layer that rung adds. The 64 B cross-check
surprised in the full decomposition and was therefore re-collected alone; its
table uses `a1_64b_recheck.txt`. The 4096 B table uses
`a1_write_pipeline.txt`.

### F1. 64-byte chunk (a typical terminal line)

| Rung | ns/record | Δ | Share of `s6` |
|---|---:|---:|---:|
| `s1_harvest` | 20.124 | — | 31.9 % |
| `s2_build` | 38.790 | **+18.666** | 29.6 % |
| `s3_frame` | 40.787 | **+1.997** | 3.2 % |
| `s4_frame_crc32` | 49.692 | **+8.905** | 14.1 % |
| `s5_buffered_file` | 62.121 | **+12.429** | 19.7 % |
| `s6_index` | 63.011 | **+0.890** | 1.4 % |
| `s7_fsync` | 4 386.7 | **+4 323.7** | (68.6× all of `s6`) |

Cross-checks: `x_crc32_only` − `s2_build` = 8.751 ns against `s4 − s3` =
8.905 ns (1.7 % apart — agreement). `x_frame_sink` = 37.975 ns against
`s2_build` = 38.790 ns — see §F3.

**Attribution at 64 B, excluding fsync:**

- application (harvest + build + index): **39.680 ns, 63.0 %**
- flatstream (framing/copy + CRC-32): **10.902 ns, 17.3 %**
- OS buffered write: **12.429 ns, 19.7 %**

### F2. 4096-byte chunk (a screen repaint)

| Rung | ns/record | Δ | Share of `s6` |
|---|---:|---:|---:|
| `s1_harvest` | 20.020 | — | 1.7 % |
| `s2_build` | 108.85 | **+88.83** | 7.4 % |
| `s3_frame` | 167.05 | **+58.20** | 4.8 % |
| `s4_frame_crc32` | 520.92 | **+353.87** | 29.4 % |
| `s5_buffered_file` | 1 203.9 | **+682.98** | 56.8 % |
| `s6_index` | 1 201.6 | unresolved (below run-to-run resolution) | unresolved |
| `s7_fsync` | 7 534.5 | **+6 332.9** | (5.3× all of `s6`) |

Cross-check: `x_crc32_only` − `s2_build` = 359.14 ns against `s4 − s3` =
353.87 ns (1.5 % apart — agreement). The index rung measured 2.3 ns faster
than the otherwise identical file rung; that impossible negative delta puts
the index cost below this wall-clock harness's resolution.

At this size the picture inverts in an instructive way. Framing's *call* overhead
is still below resolution; the `s3 − s2` = 58.20 ns rung is the payload copy into the
destination buffer (≈ 69 GB/s, a cache-resident copy). CRC-32 becomes
flatstream's dominant cost because it is a pure function of payload bytes
(≈ 11.6 GB/s here — see §F5 on hardware acceleration). And the OS write path,
also a function of bytes, takes over as the single largest term.

### F3. The noise floor, stated plainly

At 64 B, `x_frame_sink` (37.975 ns) measured **faster** than `s2_build`
(38.790 ns),
despite doing strictly more work. Their Criterion confidence intervals do not
overlap, so this is not sampling variance — it is a codegen artifact of where
`black_box` sits in each arm.

The correct reading is not "framing is free". It is: **framing's call overhead at
64 B is below what this harness can resolve**, somewhere in the interval
[0, ~2] ns/record. The positive 1.997 ns `s3 − s2` rung includes copying the
payload into the destination `Vec`; it is not a pure call-overhead result.
Any call-overhead figure quoted more precisely than that from a
wall-clock bench would be fiction. This is exactly the gap backlog item **A2**
(instruction counts via `scripts/instruction_counts.sh`) exists to close, and it
is the reason A2 should not be skipped just because A1 came out favourable.

### F4. Durability dominates everything

One `sync_data()` per 1000-record batch raises the measured batch from
63.011 µs to 4.3867 ms at 64 B and from 1.2016 ms to 7.5345 ms at 4 KiB.
The checkpoint increment is therefore **68.6×** the entire non-sync 64 B
pipeline but only **5.3×** the 4 KiB pipeline. At this specific cadence,
flatstream's framing-plus-CRC share of total elapsed time is approximately
**0.25 % (64 B) / 5.5 % (4 KiB)**.

### F5. Note on CRC-32 acceleration

`crc32fast` uses hardware acceleration **where the target provides it** —
SSE4.2/PCLMULQDQ on x86-64, the CRC32 instructions on aarch64 — and falls back to
a scalar table-driven implementation otherwise. The ≈11.6 GB/s measured at 4 KiB
in §F2 is an accelerated aarch64 path and must not be quoted as a portable
figure. Per `CONTRIBUTING.md` §6 A1, phrase this as *hardware-assisted where
available, scalar fallback otherwise* — never as universally accelerated.

## Conclusion

**The consumer's attribution is supported for the 64 B terminal-line shape,
but not as a workload-independent statement.** Application harvest/build/index
is 63.0 % of the non-durable 64 B record, versus 17.3 % for flatstream
framing/copy plus CRC-32 and 19.7 % for buffered file output. At 4 KiB, the
picture inverts: flatstream is about 34.3 %, the OS rung about 56.8 %, and
application harvest/build about 9 % (with index cost unresolved).

Framing call overhead itself remains below this wall-clock harness's
resolution; the size-dependent library cost is payload copying plus optional
CRC. A large regression therefore cannot be assigned to “flatstream” or “the
application” without matching the consumer's payload-size and durability
cadence.

### What changes as a result

- Publish the attribution only with its 64 B workload qualifier.
- **No framing-call optimization is indicated by this run.** Its call overhead
  is below the harness's measurement floor. (E1's vectored write addresses
  syscall count on unbuffered sinks, a
  different argument tracked separately in `FINDINGS_VECTORED_FRAMING.md`.)
- **A2 is promoted, not retired.** §F3 shows wall clock cannot resolve the
  framing path; the instruction-count characterization is the only way to put a
  real number on it.
- Two documentation consequences, both actioned in the same change: CRC-32 must
  be described as hardware-assisted *where available* (§F5), and the
  memory-policy guidance should note that at 4 KiB the payload copy and CRC
  dominate, so builder reclamation tuning is not where large-record throughput is
  won.

## Threats to validity

- **T1 — Single machine, and a noisy one.** All figures come from one Apple M4
  laptop. The run is committed and internally paired, but absolute values do
  not transfer across hardware. Criterion also prints changes against its
  machine-local prior baseline; those cross-run “improved/regressed” labels are
  not used anywhere in this document. The first isolated 64 B run's CRC
  cross-check differed by ~16%; per the re-collection rule it was rerun alone,
  where the two CRC estimates agreed within 1.7 %. F1 uses only that recheck.
- **T2 — Wall clock cannot resolve pure framing call overhead or the 4 KiB
  index delta.** §F3 is explicit about the former; the negative `s6 − s5`
  result is explicit about the latter. A2/instruction counts are the appropriate
  instrument.
- **T3 — One workload shape.** The terminal-journaling profile is realistic but
  singular. A workload with a costlier `StreamSerialize` body, or with mixed
  record sizes that provoke internal-builder bloat, would shift the shares toward
  the application — which strengthens rather than weakens the conclusion, but the
  specific percentages would not transfer.
- **T4 — sync cost is storage- and cadence-specific.** The measured increments
  are from this SSD/filesystem with one `sync_data()` per 1,000 records. The
  separate E3 cadence benchmark demonstrates why no single multiplier can
  describe every policy.
- **T5 — This characterizes a healthy pipeline, and does not diagnose a
  regression.** It bounds flatstream's steady-state share. Still open for a
  consumer seeing a real drop, and *not* addressed here: simple-mode
  internal-builder bloat under mixed record sizes (see
  `benches/write_path_benchmarks.rs`, which shows this is a real effect); a
  missing `BufWriter`, or an `fsync` per record rather than per batch (§F4 shows
  this is worth three orders of magnitude); a caller's `StreamSerialize` body
  allocating per record (the zero-allocation guarantee covers the frame path, not
  the caller's code); or validator adapters left installed on a hot write path.
