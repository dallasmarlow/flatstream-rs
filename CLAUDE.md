# flatstream-rs project memory

Read `docs/CONTRIBUTING.md` before changing code. It is the source of truth for
the quality bar, gate, findings format, and currently assignable work.

## Current release line

- Active integration line: `v0.2.8`.
- Wire format remains headerless v0.2 framing:
  `[u32 length][optional fixed-width checksum][payload]`.
- A core v3 preamble is owner-declined. Persistent applications must carry and
  validate their own framing/checksum/schema generation manifest.
- On-wire bytes are normative. Do not change them as incidental feature work.

## Shipped v0.2.8 primitives

- Writer and reader `FrameReceipt` positions (`end`/`range`,
  `bytes_written`/`bytes_consumed`, start offsets).
- `read_frame_at` with caller-owned scratch, one initial seek, and read-counted
  receipt length; receipt-aware forward APIs.
- Single-`write_vectored` built-in framing with partial-write handling.
- `Error` → `io::Error` preserves underlying I/O kinds and `UnexpectedEof`;
  other library/protocol failures map to `InvalidData` with the original error
  retained as the inner payload.
- Static frame/byte durability policies and durable watermarks; time-based
  checkpoints are application-scheduled manual syncs, not per-frame clock reads.
- Static memory policies; `NoSync` and `NoMemoryPolicy` are zero-sized defaults.
- Statically dispatched `PostWriteObserver` with a zero-sized default and
  explicit success/write-failure/durability-failure outcomes.
- Reader/writer direct source/sink access (`get_ref`/`get_mut`) is intentionally
  absent; consume with `into_inner` before out-of-band I/O, then reconstruct
  with the correct offset.
- Strict torn-tail recovery: only `UnexpectedEof` authorizes truncation after
  writing has stopped.

`UnexpectedEof` describes the current read attempt, not permanent source
finality. A seekable live-file follower retries `read_frame_at` from the same
absolute offset. A sequential `StreamReader` cannot continue after consuming a
partial frame.

## Non-negotiable invariants

- Zero-copy means borrowed payload access; a generic `Read` still copies once
  into the reusable reader/scratch buffer.
- Zero-allocation claims apply after buffers reach their high-water mark.
- Static dispatch is the default. Do not add boxed hot-path policy calls.
- `Write::is_write_vectored` remains unstable (`can_vector`) on stable/beta
  Rust as of 2026-07-29; do not gate framing on it or move the crate to nightly.
- Default builds forbid unsafe code; `unsafe_typed` is the sole opt-out.
- Tests/examples must assert bytes, round trips, counts, or error kinds.
- No new runtime dependency or performance claim without evidence and review.

## Verification

Run `scripts/gate.sh` before handoff. The canonical gate includes formatting,
feature-matrix Clippy/tests, examples, README doctests, rustdoc, benchmark/fuzz
compile checks, and MSRV validation.

Auxiliary verification:

- `scripts/bench_isolated.sh` for Criterion evidence.
- `scripts/instruction_counts.sh` for pinned Gungraun/Callgrind counts.
- `scripts/miri.sh` and `scripts/fuzz.sh` when the changed boundary requires it.

Never compare wall-clock values from separate runs as published evidence.
Commit the stamped raw snapshot and a completed findings document. Re-run any
surprising result.

## Current work lanes

The macOS contributor lane is complete: B3 shipped its design note, statically
dispatched post-write hook, self-asserting example/tests, and overhead findings;
E2 is declined (`docs/planning/E2_CHECKSUM_COMPOSITION.md`).

A4 is complete (2026-07-28):
`docs/benchmark/FINDINGS_COMPRESSION_FEASIBILITY.md` records substantial
modeled-payload byte savings but slower buffered-file throughput, so no
production compression adapter follows.

Maintainer/reference-machine work:

- C2 positioned-read Miri expansion.
- A3 generic `Read` copy-cost benchmark.

Do not ask a benchmark-incapable contributor to collect or interpret
performance numbers.

## Reference consumer

Palimpsest uses CRC-32 frames, external receipts, segmented files, a manifest,
strict final-segment recovery, and a bounded decoded-frame LRU. Its current
flush-only `$TMPDIR` journal promises process-crash recovery, not power-loss-safe
WAL semantics. See `ONBOARDING.md` for the exact application contract.
