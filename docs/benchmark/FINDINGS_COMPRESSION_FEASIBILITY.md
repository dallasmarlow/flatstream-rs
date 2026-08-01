# Findings: does frame-local compression pay for journal payloads?

**Author:** contributor (A4)
**Date:** 2026-07-28
**Status:** complete — seven isolated raw snapshots retained locally under
`docs/benchmark/raw/a4_*.txt`

## Hypothesis

Highly repetitive journal-like text should compress substantially, while
deterministic pseudorandom bytes should expand slightly. The hypothesis under
test is whether even the strongest byte savings repay LZ4 or low-latency
Zstandard CPU in the current buffered-file path without creating unbounded
frame latency.

## Methodology

### Environment

- rustc: `rustc 1.97.1 (8bab26f4f 2026-07-14) (Homebrew)` — the declared MSRV
- OS / arch: Darwin 25.5.0 / arm64 (macOS 26.5.1)
- CPU: Apple M4, 10 cores, 32 GiB
- Relevant dependencies: Criterion 0.5.1, `lz4_flex` 0.14.0, `zstd` 0.13.3,
  `zstd-sys` 2.0.16 + Zstandard 1.5.7, `flatbuffers` 25.12.19
- Codec settings: independent LZ4 blocks with a reused `CompressTable`;
  Zstandard bulk level 1 with reused compression/decompression contexts
- Feature flags: none
- Saved Criterion baseline: not used. Every conclusion compares arms collected
  inside one isolated `(distribution, size)` run.

**Instrument choice.** Criterion wall clock is the right instrument because the
question is end-to-end throughput and per-frame latency. The micro arms time
codec CPU only; the file arms compress (where selected), write an 8 MiB logical
batch through one reused 64 KiB `BufWriter`, and flush to the OS page cache.
There is no `fsync`: this models a flush-only process-crash contract, not a
power-loss-safe WAL.

### Inputs

Three size classes are covered: 4 KiB, 64 KiB, and 256 KiB.

1. **Compressible control:** a repeated terminal test-result line.
2. **Incompressible control:** deterministic xorshift bytes.

The benchmark prints a SHA-256 fingerprint and verifies LZ4 and Zstandard
round trips byte-for-byte before timing. All codec buffers and contexts are
created and warmed before measurement.

The file arm is deliberately payload-only. It does not invent production wire
bytes. For the size table below, "wire ratio" uses conservative benchmark
accounting solely to answer A4: current CRC-32 framing is 8 bytes
(`[stored_len][crc32]`), while the compressed side is charged 12 bytes
(`[stored_len][crc32][decoded_len]`). This is **not** a format proposal; exact
metadata and checksum coverage remain a future wire-format decision.

### Steps

`A4_CASE` selects one group before Criterion starts. This is required on this
machine: a Criterion positional filter is followed by Cargo 1.97's hidden
`--bench` argument and Criterion 0.5 rejects that order.

```bash
A4_CASE=compressible_4k scripts/bench_isolated.sh \
  a4_compressible_4k compression_feasibility '' -- --locked
A4_CASE=compressible_64k scripts/bench_isolated.sh \
  a4_compressible_64k compression_feasibility '' -- --locked
A4_CASE=compressible_256k scripts/bench_isolated.sh \
  a4_compressible_256k compression_feasibility '' -- --locked

A4_CASE=incompressible_4k scripts/bench_isolated.sh \
  a4_incompressible_4k compression_feasibility '' -- --locked
A4_CASE=incompressible_64k scripts/bench_isolated.sh \
  a4_incompressible_64k compression_feasibility '' -- --locked
A4_CASE=incompressible_256k scripts/bench_isolated.sh \
  a4_incompressible_256k compression_feasibility '' -- --locked

# Closest result to break-even; required surprising-result recheck.
A4_CASE=compressible_64k scripts/bench_isolated.sh \
  a4_compressible_64k_recheck compression_feasibility '' -- --locked
```

## Findings

All times are Criterion medians with the 95% confidence interval in brackets.
Cross-run "change" labels in the raw files are ignored.

### F1. Stored bytes depend almost entirely on payload distribution

| Distribution | Input | LZ4 bytes / wire ratio | Zstd-1 bytes / wire ratio |
|---|---:|---:|---:|
| compressible | 4,096 B | 84 / 0.023 | 76 / 0.021 |
| compressible | 65,536 B | 325 / 0.005 | 77 / 0.001 |
| compressible | 262,144 B | 1,096 / 0.004 | 91 / <0.001 |
| incompressible | 4,096 B | 4,114 / 1.005 | 4,106 / 1.003 |
| incompressible | 65,536 B | 65,794 / 1.004 | 65,546 / 1.000 |
| incompressible | 262,144 B | 263,173 / 1.004 | 262,159 / 1.000 |

The compressible control nearly vanishes. Incompressible data expands, proving
that any future format needs an explicit "store raw when compression does not
win" representation; blindly compressing every frame is not viable.

### F2. Codec latency is bounded at these frame sizes, but content-sensitive

Each cell is `encode / decode` in microseconds.

| Distribution | Input | LZ4 median [95% CI] | Zstd-1 median [95% CI] |
|---|---:|---:|---:|
| compressible | 4 KiB | 0.254 [0.254, 0.255] / 0.206 [0.206, 0.206] | 0.645 [0.644, 0.646] / 0.264 [0.263, 0.264] |
| compressible | 64 KiB recheck | 2.323 [2.319, 2.328] / 3.714 [3.708, 3.723] | 2.626 [2.623, 2.629] / 3.707 [3.696, 3.718] |
| compressible | 256 KiB | 8.279 [8.258, 8.305] / 15.670 [15.633, 15.728] | 10.076 [10.065, 10.086] / 15.468 [15.454, 15.484] |
| incompressible | 4 KiB | 0.484 [0.483, 0.484] / 0.045 [0.045, 0.045] | 1.361 [1.359, 1.362] / 0.053 [0.053, 0.053] |
| incompressible | 64 KiB | 2.415 [2.413, 2.417] / 0.634 [0.633, 0.636] | 6.125 [6.117, 6.133] / 0.572 [0.572, 0.573] |
| incompressible | 256 KiB | 7.158 [7.151, 7.167] / 3.445 [3.439, 3.453] | 18.222 [18.180, 18.266] / 3.185 [3.182, 3.188] |

On this M4, the worst measured control cost is 18.27 µs encode and 15.73 µs
decode at the confidence-interval edge. This is not a portable hard bound;
payload distribution and frame size both affect codec cost.

The uncompressed micro arm is identity borrowed-slice access and measures at
the ~0.6 ns harness floor. It is intentionally not reported as a bandwidth
claim; A3 separately measures the generic `Read` copy.

### F3. Saved bytes do not repay CPU in the current buffered-file path

Each file arm processes approximately 8 MiB of logical input. Factors are
compressed median divided by the paired uncompressed median; values above 1
are slower.

| Distribution | Input | Uncompressed | LZ4 / factor | Zstd-1 / factor |
|---|---:|---:|---:|---:|
| compressible | 4 KiB | 0.433 ms [0.432, 0.435] | 0.565 ms [0.564, 0.566] / 1.30× | 1.330 ms [1.324, 1.337] / 3.07× |
| compressible | 64 KiB recheck | 0.264 ms [0.260, 0.269] | 0.317 ms [0.313, 0.321] / 1.20× | 0.351 ms [0.347, 0.354] / 1.33× |
| compressible | 256 KiB | 0.206 ms [0.206, 0.207] | 0.267 ms [0.267, 0.268] / 1.30× | 0.325 ms [0.325, 0.325] / 1.57× |
| incompressible | 4 KiB | 0.424 ms [0.423, 0.426] | 1.544 ms [1.541, 1.547] / 3.64× | 3.543 ms [3.531, 3.563] / 8.35× |
| incompressible | 64 KiB | 0.266 ms [0.263, 0.273] | 0.815 ms [0.812, 0.817] / 3.06× | 1.223 ms [1.218, 1.232] / 4.59× |
| incompressible | 256 KiB | 0.209 ms [0.209, 0.210] | 0.495 ms [0.494, 0.497] / 2.37× | 0.876 ms [0.873, 0.880] / 4.19× |

The strongest possible byte-saving control still loses. At 64 KiB it stores
only 0.50% (LZ4) / 0.12% (Zstd) of the payload, yet the first run was 17% / 30%
slower and the required recheck was 20% / 33% slower. The direction survives.

## Conclusion

The hypothesis fails for this payload-only buffered, flush-only model:
**even extreme byte savings do not repay codec CPU.** Neither LZ4 nor
Zstandard level 1 improves end-to-end throughput on the compressible control.

The answer is distribution-dependent enough that transparent core compression
would be the wrong next change:

- repeated text retains less than 2.4%;
- incompressible data expands and burns CPU.

**No production `CompressionFramer`, runtime codec dependency, or wire change
follows from A4.** Any application-owned explicit format experiment should
start with representative production-frame traces and a raw fallback. Before
shipping, it still needs decoded-size bounds,
decompression-bomb protection, per-frame codec/raw metadata, checksum-coverage
semantics, and a manifest generation bump.

## Threats to validity

- **T1 — Controls, not production captures.** The two deterministic
  distributions intentionally bracket extreme compressibility. They do not
  establish the size or latency distribution of a real application workload.
- **T2 — Page cache, not storage media.** The file arm rewrites a warm tempfile
  and calls `flush`, not `sync_data`. It measures the current process-crash
  contract and immediate worker cost. It does not measure physical SSD
  writeback, power-loss durability, network storage, or a throttled sink.
- **T3 — One repeated frame per 8 MiB batch.** Codec blocks are independent and
  contexts reset correctly, but the source stays cache-hot. A real harvest has
  multiple distinct frames and may see different cache and branch behavior.
- **T4 — Single machine.** Absolute times are Apple-M4/macOS results. Only arms
  inside one isolated run are compared. Criterion's saved-baseline "change"
  labels are ignored.
- **T5 — Codec and backend specificity.** These results pin `lz4_flex` 0.14 and
  Zstandard 1.5.7 level 1. Different implementations, levels, dictionaries,
  architectures, or compiler versions can change both ratio and CPU cost.
- **T6 — Identity is not A3.** Uncompressed encode/decode micro arms model the
  current no-transform borrowed-slice boundary. They do not include the generic
  `Read` → reusable-buffer copy; A3 owns that measurement.
- **T7 — Reuse by construction, not an allocation claim.** Caller output
  buffers, the LZ4 table, Zstandard contexts, and `BufWriter` are reused and
  warmed. This experiment does not install an allocator counter inside either
  codec and makes no zero-allocation claim about their internals.
- **T8 — Benchmark wire accounting only.** The 8-byte/12-byte size model makes
  overhead explicit but does not settle a compressed wire format. The file
  throughput arm omits framing and checksums entirely; checksum placement could
  add a distribution-dependent cost and remains a normative format decision.
