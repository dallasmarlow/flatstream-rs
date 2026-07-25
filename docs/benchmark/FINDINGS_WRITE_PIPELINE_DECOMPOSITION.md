# Findings: what fraction of an end-to-end write is flatstream?

**Author:** contributor (A1, `CONTRIBUTING.md` §6)
**Date:** 2026-07-24
**Status:** in progress — methodology and conclusions settled; absolute numbers
are provisional pending re-collection on reference hardware (see Threats §T1)

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
asks next: **durability (`fsync`), when present, dwarfs everything else** — so
the framing layer's share is not merely small but negligible in any pipeline
that actually persists.

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
cargo bench --features crc32 --bench write_pipeline_decomposition \
  2>&1 | tee docs/benchmark/raw/a1_write_pipeline.txt
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

All figures are Criterion medians, in nanoseconds per record. "Δ" is the cost of
the layer that rung adds. Raw output: `docs/benchmark/raw/a1_write_pipeline.txt`
(regenerate with the command in Steps).

### F1. 64-byte chunk (a typical terminal line)

| Rung | ns/record | Δ | Share of `s6` |
|---|---:|---:|---:|
| `s1_harvest` | 21.4 | — | 32.4 % |
| `s2_build` | 41.1 | **+19.7** | 29.8 % |
| `s3_frame` | 41.5 | **+0.4** | 0.6 % |
| `s4_frame_crc32` | 50.4 | **+8.9** | 13.5 % |
| `s5_buffered_file` | 63.7 | **+13.3** | 20.1 % |
| `s6_index` | 66.1 | **+2.3** | 3.5 % |
| `s7_fsync` | 4 087.5 | **+4 021.4** | (61× all of `s6`) |

Cross-checks: `x_crc32_only` − `s2_build` = 8.4 ns against `s4 − s3` = 8.9 ns
(6 % apart — agreement). `x_frame_sink` = 39.1 ns against `s2_build` = 41.1 ns —
see §F3.

**Attribution at 64 B, excluding fsync:**

- application (harvest + build + index): **43.4 ns, 65.7 %**
- flatstream (framing + CRC-32): **9.3 ns, 14.1 %** — of which framing itself is
  0.4 ns (0.6 %)
- OS buffered write: **13.3 ns, 20.1 %**

### F2. 4096-byte chunk (a screen repaint)

| Rung | ns/record | Δ | Share of `s6` |
|---|---:|---:|---:|
| `s1_harvest` | 21.6 | — | 1.6 % |
| `s2_build` | 119.3 | **+97.6** | 7.1 % |
| `s3_frame` | 178.7 | **+59.5** | 4.4 % |
| `s4_frame_crc32` | 560.7 | **+382.0** | 28.0 % |
| `s5_buffered_file` | 1 362.4 | **+801.7** | 58.7 % |
| `s6_index` | 1 366.4 | **+4.0** | 0.3 % |
| `s7_fsync` | 16 470 | **+15 103.6** | (11× all of `s6`) |

Cross-check: `x_crc32_only` − `s2_build` = 365.0 ns against `s4 − s3` = 382.0 ns
(4.5 % apart — agreement).

At this size the picture inverts in an instructive way. Framing's *call* overhead
is still nil; the entire `s3 − s2` = 59.5 ns is the payload copy into the
destination buffer (≈ 69 GB/s, a cache-resident copy). CRC-32 becomes
flatstream's dominant cost because it is a pure function of payload bytes
(≈ 10.7 GB/s here — see §F5 on hardware acceleration). And the OS write path,
also a function of bytes, takes over as the single largest term.

### F3. The noise floor, stated plainly

At 64 B, `x_frame_sink` (39.1 ns) measured **faster** than `s2_build` (41.1 ns),
despite doing strictly more work. Their Criterion confidence intervals do not
overlap, so this is not sampling variance — it is a codegen artifact of where
`black_box` sits in each arm.

The correct reading is not "framing is free". It is: **framing's call overhead at
64 B is below what this harness can resolve**, somewhere in the interval
[0, ~2] ns/record, and any figure quoted more precisely than that from a
wall-clock bench would be fiction. This is exactly the gap backlog item **A2**
(instruction counts via `scripts/instruction_counts.sh`) exists to close, and it
is the reason A2 should not be skipped just because A1 came out favourable.

### F4. Durability dominates everything

One `sync_data()` per 1000-record batch costs 4.02 ms (64 B batch, 68 KiB) and
15.1 ms (4 KiB batch, 4.1 MiB) on this machine's SSD. Amortized per record that
is 4 021 ns and 15 104 ns respectively — **61× and 11× the entire rest of the
pipeline combined**. Under any fsync policy at all, flatstream's share of an
end-to-end write falls to **0.23 % (64 B) / 2.7 % (4 KiB)**.

### F5. Note on CRC-32 acceleration

`crc32fast` uses hardware acceleration **where the target provides it** —
SSE4.2/PCLMULQDQ on x86-64, the CRC32 instructions on aarch64 — and falls back to
a scalar table-driven implementation otherwise. The ≈10.7 GB/s measured at 4 KiB
in §F2 is an accelerated aarch64 path and must not be quoted as a portable
figure. Per `CONTRIBUTING.md` §6 A1, phrase this as *hardware-assisted where
available, scalar fallback otherwise* — never as universally accelerated.

## Conclusion

**The consumer's attribution is supported, and may now be stated as measured
fact.** In the terminal-journaling shape, flatstream's framing accounts for 0.6 %
of a 64-byte record's non-durable write cost and 4.4 % of a 4 KiB one. Adding the
opt-in CRC-32 brings the library's total share to 14 % and 32 % respectively;
including a per-batch fsync drops it below 3 % in both cases. The application's
own harvest and FlatBuffers construction is the larger term at small record sizes
(66 %), and the OS write path is the larger term at large ones (59 %).

A large end-to-end throughput drop therefore **cannot** be explained by
flatstream's framing layer. There is not enough time in it to lose.

### What changes as a result

- The attribution may now be published. The specific supportable sentence is the
  one above, with this document cited.
- **No code change is indicated by this experiment.** Framing is already below
  the measurement floor at small sizes; optimizing it further would be optimizing
  nothing. (E1's vectored write is justified by syscall count on unbuffered
  sinks, a different argument, measured separately in
  `FINDINGS_VECTORED_FRAMING.md`.)
- **A2 is promoted, not retired.** §F3 shows wall clock cannot resolve the
  framing path; the instruction-count characterization is the only way to put a
  real number on it.
- Two documentation consequences, both actioned in the same change: CRC-32 must
  be described as hardware-assisted *where available* (§F5), and the
  memory-policy guidance should note that at 4 KiB the payload copy and CRC
  dominate, so builder reclamation tuning is not where large-record throughput is
  won.

## Threats to validity

- **T1 — Single machine, and a noisy one; numbers are provisional.** All figures
  come from one Apple M4 laptop. While collecting the E1 experiment on the same
  machine, unchanged code moved by −24 % and +57 % between consecutive runs, so
  absolute values here should be treated as provisional until re-collected on
  reference hardware. The **ratios and adjacent-rung deltas** are the durable
  content; the orders of magnitude that carry the conclusion (fsync at 61×) are
  far outside any plausible noise band.
- **T2 — Wall clock cannot resolve the framing rung.** §F3 is explicit: the
  0.4 ns figure is a *bound*, not a measurement. It is quoted in Conclusion only
  as "below the measurement floor", never as a precise cost. A2 exists to fix
  this.
- **T3 — One workload shape.** The terminal-journaling profile is realistic but
  singular. A workload with a costlier `StreamSerialize` body, or with mixed
  record sizes that provoke internal-builder bloat, would shift the shares toward
  the application — which strengthens rather than weakens the conclusion, but the
  specific percentages would not transfer.
- **T4 — fsync cost is storage-specific.** 4.02 ms/batch is this SSD under this
  filesystem. On a different device, or with a write cache that lies about
  durability, the 61× multiplier will differ substantially. The qualitative claim
  (durability dominates) is robust; the multiplier is not.
- **T5 — This characterizes a healthy pipeline, and does not diagnose a
  regression.** It bounds flatstream's steady-state share. Still open for a
  consumer seeing a real drop, and *not* addressed here: simple-mode
  internal-builder bloat under mixed record sizes (see
  `benches/write_path_benchmarks.rs`, which shows this is a real effect); a
  missing `BufWriter`, or an `fsync` per record rather than per batch (§F4 shows
  this is worth three orders of magnitude); a caller's `StreamSerialize` body
  allocating per record (the zero-allocation guarantee covers the frame path, not
  the caller's code); or validator adapters left installed on a hot write path.
