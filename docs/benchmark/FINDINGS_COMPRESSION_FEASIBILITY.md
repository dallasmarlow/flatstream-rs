# Findings: does frame-local compression pay for journal payloads?

**Author:** contributor (A4, `CONTRIBUTING.md` §6)  
**Date:** 2026-07-28  
**Status:** complete — ten isolated raw snapshots committed under
`docs/benchmark/raw/a4_*.txt`

## Hypothesis

Palimpsest's row-oriented FlatBuffers should contain enough repeated table,
string, and style-run structure for frame-local compression to reduce stored
bytes materially. The hypothesis under test is stronger and can fail:
**for representative 64–256 KiB journal frames, those saved bytes repay LZ4 or
low-latency Zstandard CPU in the current buffered-file path without creating
unbounded frame latency.**

The controls establish the distribution limits. Repeated terminal text should
be the best case; deterministic pseudorandom bytes should expand slightly and
make every compression path lose.

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
There is no `fsync`: that matches Palimpsest's current process-crash contract,
not a power-loss-safe WAL.

### Inputs

Three size classes are covered: 4 KiB, 64 KiB, and approximately 256 KiB.

1. **Palimpsest:** committed FlatBuffers produced by Palimpsest's actual
   `Frame { first_seq, rows, mean_ink }` encoder at consumer revision
   `987b3b3df16059343e00225cb132d9e5f462fd65`. The deterministic source rows
   model build/test output with row-unique crate names, paths, positions,
   Unicode, color/style runs, wrapping, and occasional combining marks. They
   are schema-exact modeled fixtures, not captured user data. The 4 KiB fixture
   is a 16-row partial frame; the larger fixtures contain the normal 256 rows.
2. **Compressible control:** a repeated terminal test-result line.
3. **Incompressible control:** deterministic xorshift bytes.

The benchmark prints a SHA-256 fingerprint and verifies LZ4 and Zstandard
round trips byte-for-byte before timing. All codec buffers and contexts are
created and warmed before measurement. The Palimpsest fixture fingerprints are:

- 4 KiB: 4,080 B,
  `f043cf145ffd29dd37ff5238d266fb44219e7085b5696d30d7267873f766f1e4`
- 64 KiB: 65,168 B,
  `07fb3981f8fd747ecc2aa88e89dcc42ded33f75def360c4331e48ba51fb8e553`
- 256 KiB: 262,272 B,
  `e8fcbc50505836b2c4c194925193a21346d71e8a40aacf35ae956f8900cc71c4`

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
A4_CASE=palimpsest_4k scripts/bench_isolated.sh \
  a4_palimpsest_4k compression_feasibility '' -- --locked
A4_CASE=palimpsest_64k scripts/bench_isolated.sh \
  a4_palimpsest_64k compression_feasibility '' -- --locked
A4_CASE=palimpsest_256k scripts/bench_isolated.sh \
  a4_palimpsest_256k compression_feasibility '' -- --locked

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
Cross-run "change" labels in the raw files are ignored. The three Palimpsest
fixtures were refined after an initial pass, so those labels compare different
inputs and are especially meaningless.

### F1. Stored bytes depend almost entirely on payload distribution

| Distribution | Input | LZ4 bytes / wire ratio | Zstd-1 bytes / wire ratio |
|---|---:|---:|---:|
| Palimpsest | 4,080 B | 2,232 / 0.549 | 1,505 / 0.371 |
| Palimpsest | 65,168 B | 29,383 / 0.451 | 17,612 / 0.270 |
| Palimpsest | 262,272 B | 86,061 / 0.328 | 60,972 / 0.233 |
| compressible | 4,096 B | 84 / 0.023 | 76 / 0.021 |
| compressible | 65,536 B | 325 / 0.005 | 77 / 0.001 |
| compressible | 262,144 B | 1,096 / 0.004 | 91 / <0.001 |
| incompressible | 4,096 B | 4,114 / 1.005 | 4,106 / 1.003 |
| incompressible | 65,536 B | 65,794 / 1.004 | 65,546 / 1.000 |
| incompressible | 262,144 B | 263,173 / 1.004 | 262,159 / 1.000 |

The modeled Palimpsest frames retain **23–55%** of current wire bytes,
with Zstandard level 1 smaller in every class. The compressible control nearly
vanishes. Incompressible data expands, proving that any future format needs an
explicit "store raw when compression does not win" representation; blindly
compressing every frame is not viable.

### F2. Codec latency is bounded at these frame sizes, but content-sensitive

Each cell is `encode / decode` in microseconds.

| Distribution | Input | LZ4 median [95% CI] | Zstd-1 median [95% CI] |
|---|---:|---:|---:|
| Palimpsest | 4 KiB | 2.949 [2.940, 2.961] / 0.727 [0.725, 0.728] | 7.957 [7.941, 7.976] / 3.567 [3.553, 3.585] |
| Palimpsest | 64 KiB | 46.041 [45.960, 46.131] / 12.713 [12.698, 12.728] | 62.480 [62.385, 62.582] / 28.120 [28.072, 28.174] |
| Palimpsest | 256 KiB | 119.10 [118.85, 119.36] / 29.437 [29.407, 29.467] | 228.82 [228.23, 229.51] / 94.176 [94.094, 94.257] |
| compressible | 4 KiB | 0.254 [0.254, 0.255] / 0.206 [0.206, 0.206] | 0.645 [0.644, 0.646] / 0.264 [0.263, 0.264] |
| compressible | 64 KiB recheck | 2.323 [2.319, 2.328] / 3.714 [3.708, 3.723] | 2.626 [2.623, 2.629] / 3.707 [3.696, 3.718] |
| compressible | 256 KiB | 8.279 [8.258, 8.305] / 15.670 [15.633, 15.728] | 10.076 [10.065, 10.086] / 15.468 [15.454, 15.484] |
| incompressible | 4 KiB | 0.484 [0.483, 0.484] / 0.045 [0.045, 0.045] | 1.361 [1.359, 1.362] / 0.053 [0.053, 0.053] |
| incompressible | 64 KiB | 2.415 [2.413, 2.417] / 0.634 [0.633, 0.636] | 6.125 [6.117, 6.133] / 0.572 [0.572, 0.573] |
| incompressible | 256 KiB | 7.158 [7.151, 7.167] / 3.445 [3.439, 3.453] | 18.222 [18.180, 18.266] / 3.185 [3.182, 3.188] |

On this M4, the worst measured single-frame cost is 229.51 µs encode and
94.26 µs decode at the confidence-interval edge. That supports "sub-millisecond
for these fixtures on this machine," not a portable hard bound. The large
difference between modeled and repeated text also shows that frame size alone
does not predict CPU cost.

The uncompressed micro arm is identity borrowed-slice access and measures at
the ~0.6 ns harness floor. It is intentionally not reported as a bandwidth
claim; A3 separately measures the generic `Read` copy.

### F3. Saved bytes do not repay CPU in the current buffered-file path

Each file arm processes approximately 8 MiB of logical input. Factors are
compressed median divided by the paired uncompressed median; values above 1
are slower.

| Distribution | Input | Uncompressed | LZ4 / factor | Zstd-1 / factor |
|---|---:|---:|---:|---:|
| Palimpsest | 4 KiB | 0.389 ms [0.388, 0.390] | 6.551 ms [6.526, 6.595] / 16.9× | 16.942 ms [16.911, 16.978] / 43.6× |
| Palimpsest | 64 KiB | 0.469 ms [0.467, 0.471] | 7.062 ms [6.758, 7.465] / 15.1× | 8.363 ms [8.350, 8.378] / 17.9× |
| Palimpsest | 256 KiB | 0.207 ms [0.207, 0.207] | 3.901 ms [3.887, 3.918] / 18.9× | 7.354 ms [7.335, 7.379] / 35.5× |
| compressible | 4 KiB | 0.433 ms [0.432, 0.435] | 0.565 ms [0.564, 0.566] / 1.30× | 1.330 ms [1.324, 1.337] / 3.07× |
| compressible | 64 KiB recheck | 0.264 ms [0.260, 0.269] | 0.317 ms [0.313, 0.321] / 1.20× | 0.351 ms [0.347, 0.354] / 1.33× |
| compressible | 256 KiB | 0.206 ms [0.206, 0.207] | 0.267 ms [0.267, 0.268] / 1.30× | 0.325 ms [0.325, 0.325] / 1.57× |
| incompressible | 4 KiB | 0.424 ms [0.423, 0.426] | 1.544 ms [1.541, 1.547] / 3.64× | 3.543 ms [3.531, 3.563] / 8.35× |
| incompressible | 64 KiB | 0.266 ms [0.263, 0.273] | 0.815 ms [0.812, 0.817] / 3.06× | 1.223 ms [1.218, 1.232] / 4.59× |
| incompressible | 256 KiB | 0.209 ms [0.209, 0.210] | 0.495 ms [0.494, 0.497] / 2.37× | 0.876 ms [0.873, 0.880] / 4.19× |

The strongest possible byte-saving control still loses. At 64 KiB it stores
only 0.50% (LZ4) / 0.12% (Zstd) of the payload, yet the first run was 17% / 30%
slower and the required recheck was 20% / 33% slower. The direction survives.

For the modeled journal data, a CPU-only break-even estimate
`saved_bytes / encode_time` ranges from approximately 0.58–1.38 GiB/s for LZ4
and 0.30–0.82 GiB/s for Zstd. A genuinely bandwidth-limited sink below those
rates might repay encode CPU, but this experiment did not test such a sink.
Palimpsest's current flush-to-page-cache path is far above those thresholds,
so compression adds immediate worker latency even while reducing eventual
writeback and disk occupancy.

## Conclusion

The hypothesis fails for Palimpsest's current write contract: **saved bytes do
not repay codec CPU in the buffered, flush-only path.** Frame-local latency is
bounded and storage savings are substantial on the modeled journal fixtures,
but neither LZ4 nor Zstandard level 1 improves end-to-end throughput. Even the
best-case repeated-text control remains slower.

The answer is distribution-dependent enough that transparent core compression
would be the wrong next change:

- modeled Palimpsest frames retain 23–55% of current wire bytes;
- repeated text retains less than 2.4%;
- incompressible data expands and burns CPU.

**No production `CompressionFramer`, runtime codec dependency, or wire change
follows from A4.** If Palimpsest later values disk budget or physical writeback
more than worker latency, an application-owned explicit format experiment is
reasonable, starting with real anonymized production-frame traces and a raw
fallback. Before shipping, it still needs decoded-size bounds,
decompression-bomb protection, per-frame codec/raw metadata, checksum-coverage
semantics, and a manifest generation bump.

## Threats to validity

- **T1 — Modeled consumer frames, not production captures.** The fixtures use
  Palimpsest's exact schema and encoder but deterministic build/test rows.
  They avoid private user data and are reproducible, but their 23–55% ratios
  must not be presented as a production distribution. Real anonymized traces
  are the next evidence step before any format work.
- **T2 — Page cache, not storage media.** The file arm rewrites a warm tempfile
  and calls `flush`, not `sync_data`. It measures the current process-crash
  contract and immediate worker cost. It does not measure physical SSD
  writeback, power-loss durability, network storage, or a throttled sink.
- **T3 — One repeated frame per 8 MiB batch.** Codec blocks are independent and
  contexts reset correctly, but the source stays cache-hot. A real harvest has
  multiple distinct frames and may see different cache and branch behavior.
- **T4 — Single machine.** Absolute times are Apple-M4/macOS results. Only arms
  inside one isolated run are compared. Criterion's saved-baseline "change"
  labels are ignored; for refined Palimpsest fixtures they compare different
  bytes under the same benchmark ID.
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
