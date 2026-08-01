# Findings: what does per-frame position accounting cost?

**Author:** A2 position-accounting characterization
**Date:** 2026-07-25
**Status:** historical characterization — pinned raw output retained locally as
`docs/benchmark/raw/a2_position_accounting.txt`. The reader results remained
stable, but final writer poisoning/exact partial-write accounting changed
writer codegen enough that the writer deltas below are superseded and must not
be quoted for the release build.

## Hypothesis

`StreamWriter` and `StreamReader` layer position accounting on top of the raw
framing work: a `CountingWriter`/`CountingReader` that adds every byte the sink
accepts or the source returns, `bytes_written`/`bytes_consumed` arithmetic, and
the construction of a `FrameReceipt`/`ReadFrame` per frame. v0.2.8's wall-clock
runs resolved *no* forward-read regression from this (see
`FINDINGS_POSITIONED_READS.md`), but wall-clock cannot resolve a per-frame cost
of a few instructions — it sits inside the documented −24%/+57% drift band.
This experiment upgrades that null result to a *counted* one.

The claims to prove or falsify, per frame, in the pinned environment:

1. The byte counter and receipt arithmetic add a small, **nonzero** instruction
   count over a framing-only baseline that does no accounting.
2. The cost a caller *ignoring* receipts pays (`write_finished` /
   `process_all`) is at or below the cost of consuming them
   (`*_with_receipt`), because the
   discarded receipt's arithmetic is a dead-code-elimination candidate while the
   byte counter's increments are not.
3. The accounting cost is roughly constant across framing schemes (default vs
   CRC-32) — it is per-I/O-call bookkeeping, independent of checksum width — so
   it is a *smaller fraction* of a CRC-32 frame than of a default frame.

If accounting turns out to be free (fully elided even when the receipt is
consumed) or, conversely, expensive enough to matter against real framing, that
falsifies (1)/(2) and is the finding.

## Methodology

Instruction counts (Gungraun/callgrind), not wall-clock: a per-frame delta of a
handful of instructions is exactly what wall-clock on this workstation cannot
resolve, and what callgrind can. Counts are comparable **only** within one
pinned toolchain/dependency/target/flags environment; the container in
`scripts/instruction_counts.sh` pins it and its `rustc -Vv` / valgrind /
gungraun-runner fingerprint is recorded in the raw snapshot header.

### Isolation design

The end-to-end arms already in this bench (`write_default`, `read_default`, …)
measure builder + framing + accounting together and cannot separate the last
term. A2 isolates accounting by **pairing** each accounted path against a
`*_direct` twin that does byte-identical serialize/frame/checksum work but drives
the `Framer`/`Deframer` straight over a raw `Cursor` — no `CountingWriter`/
`CountingReader`, no `bytes_*` arithmetic, no `FrameReceipt`/`ReadFrame`. The
instruction delta between a twin and its accounted sibling is the accounting
cost alone; the shared builder/framing/checksum work cancels.

Three arms per framing scheme, per direction:

| Arm | Path | What it isolates |
|---|---|---|
| `*_direct` | `Framer::frame_and_write` / `Deframer::read_and_deframe` over a bare `Cursor` | framing only — accounting-free baseline |
| `write_accounted` / `write_crc32` / `read_accounted` / `read_crc32` | `write_finished` / `process_all` | the common accounted path: counter increments, receipt discarded (DCE candidate) |
| `*_receipt` | `write_finished_with_receipt` / `process_all_with_receipt`, receipt `black_box`ed | full accounting: counter **and** un-elidable receipt math |

Both schemes covered: **default** (`write_direct`/`write_accounted`/
`write_receipt`, `read_direct`/`read_accounted`/`read_receipt`) and **CRC-32**
(`write_crc32_direct`/`write_crc32`/
`write_crc32_receipt`, `read_crc32_direct`/`read_crc32`/`read_crc32_receipt`).

CRC-32 is used for the checksummed scheme rather than XXH3-64 so the accounting
delta is read against a checksum whose own per-frame cost is well characterized
elsewhere; the `write_xxhash64`/`read_xxhash64` arms are retained unchanged as
the E3/E4 continuity baseline. CRC-32 is hardware-assisted only where
SSE4.2/PCLMULQDQ is available (scalar fallback otherwise); the pinned
container's fingerprint determines which path ran.

All arms serialize the same 100 `TelemetryEvent` frames (24-byte payloads,
stack-staged so no allocation lands inside the measured loop) and reuse one
builder / one buffer, matching the existing suite. Read arms consume payload
bytes through `black_box` so LLVM cannot erase the read; write arms `black_box`
the item and (for the receipt arms) the returned receipt.

Writer benchmark setup also constructs direct and accounted default/CRC-32
streams and asserts byte-for-byte equality before any measured arm runs. The
direct/accounted loops apply `black_box` at the same event boundary; no
baseline-only payload barrier contaminates the instruction delta.

Reader arms all call the same non-inlined payload consumer and return its
accumulator. Receipt arms additionally fold receipt fields into an opaque return
tuple, preventing LLVM from erasing either payload reads or receipt arithmetic.

### Environment

- rustc: `rustc 1.97.1 (8bab26f4f 2026-07-14)`
- OS / arch: Linux / `aarch64-unknown-linux-gnu` container
- CPU: aarch64 host; model was not emitted by the runner
- Relevant dependency versions: flatbuffers 25.12.19, crc32fast 1.5.0 (from `Cargo.lock`)
- Tool: gungraun 0.19.4 via `scripts/instruction_counts.sh`
- Feature flags: `--features instruction_bench,all_checksums`
- Baseline compared against: within-run `*_direct` twins (not a prior run)

### Steps

```bash
# One pinned run produces every arm; the paired deltas come from a single run,
# never across runs.
scripts/instruction_counts.sh | tee docs/benchmark/raw/a2_position_accounting.txt
```

## Findings

All counts below are from the same pinned run. Per-frame deltas divide the
100-frame difference by 100.

### F1. Writer accounting — default

| Arm | Instructions / 100 frames | Δ vs `write_direct` | Per frame |
|---|---:|---:|---:|
| `write_direct` (baseline) | 26,130 | — | — |
| `write_accounted` (counter, receipt discarded) | 26,233 | +103 | **+1.03** |
| `write_receipt` (full receipt) | 26,740 | +610 | **+6.10** |

Consuming the default receipt adds 507 instructions beyond the discarded path,
or **5.07/frame**.

### F2. Writer accounting — CRC-32

| Arm | Instructions / 100 frames | Δ vs `write_crc32_direct` | Per frame |
|---|---:|---:|---:|
| `write_crc32_direct` (baseline) | 39,305 | — | — |
| `write_crc32` (counter, receipt discarded) | 39,529 | +224 | **+2.24** |
| `write_crc32_receipt` (full receipt) | 39,833 | +528 | **+5.28** |

Consuming the CRC-32 receipt adds 304 instructions beyond the discarded path,
or **3.04/frame**.

### F3. Reader accounting — default

| Arm | Instructions / 100 frames | Δ vs `read_direct` | Per frame |
|---|---:|---:|---:|
| `read_direct` (baseline) | 19,552 | — | — |
| `read_accounted` (counter, receipt discarded) | 29,168 | +9,616 | **+96.16** |
| `read_receipt` (full receipt) | 29,476 | +9,924 | **+99.24** |

Consuming the default read receipt adds 308 instructions beyond the discarded
path, or **3.08/frame**. Most of the reader delta is the counted `Read` wrapper
and `StreamReader` boundary, not receipt consumption.

### F4. Reader accounting — CRC-32

| Arm | Instructions / 100 frames | Δ vs `read_crc32_direct` | Per frame |
|---|---:|---:|---:|
| `read_crc32_direct` (baseline) | 34,684 | — | — |
| `read_crc32` (counter, receipt discarded) | 40,075 | +5,391 | **+53.91** |
| `read_crc32_receipt` (full receipt) | 40,481 | +5,797 | **+57.97** |

Consuming the CRC-32 read receipt adds 406 instructions beyond the discarded
path, or **4.06/frame**.

## Conclusion

Position accounting is nonzero but remains small on writes: **1.03–2.24
instructions/frame** when receipts are discarded and **5.28–6.10/frame** when
fully consumed. Reader accounting is larger: **53.91–96.16
instructions/frame** before explicit receipt consumption, while consuming the
receipt itself adds only **3.08–4.06/frame**.

Hypothesis 1 held for writers but “small” did not describe the whole reader
wrapper delta. Hypothesis 2 held in all four pairs: discarded receipts cost less
than consumed receipts. Hypothesis 3 was falsified for total accounting
(default and CRC-32 reader deltas differ materially), though the incremental
cost of consuming a receipt stays in the narrow 3–5 instruction range.

Nothing changes in the public API. The wall-clock positioned-read benchmark
resolved no forward regression, and receipts eliminate application wire
arithmetic and per-lookup allocation. A separate uncounted reader type is not
justified by these counts; the result closes A2 as characterization, not an
optimization request.

## Threats to validity

- **Single pinned environment.** Instruction counts are valid only in the
  recorded container/toolchain; a different `rustc`/LLVM shifts absolute values.
  Only the within-run paired deltas are portable in meaning, not the absolutes.
- **`*_direct` is a modelled baseline, not a shipped path.** It reuses one
  builder and one `Cursor<&mut Vec<u8>>` sink to mirror the accounted arms as
  closely as possible, but it is bench code; if it drifts from what the
  `StreamWriter`/`StreamReader` actually do around framing, the isolated delta
  absorbs that drift. Writer setup asserts direct/accounted byte parity before
  measurement, and all read arms use the same non-inlined payload consumer, but
  function-boundary codegen can still differ.
- **DCE is compiler-dependent.** The claim that the discarded-receipt path costs
  ≤ the consumed-receipt path relies on LLVM eliminating dead receipt
  arithmetic; a different optimizer might not, which would itself be the finding
  rather than an error.
- **Mock sink, not a syscall.** Framing writes into an in-memory `Cursor`, so
  these counts are the library's own accounting instructions, not I/O. Do not
  infer a production percentage from them — that is A1's job, and this doc makes
  no end-to-end attribution.
- **24-byte uniform payloads.** Accounting is per-frame and per-I/O-call, so it
  is largely payload-size-independent; but a workload with wildly varying sizes
  exercises buffer growth the steady-state loop here does not.
