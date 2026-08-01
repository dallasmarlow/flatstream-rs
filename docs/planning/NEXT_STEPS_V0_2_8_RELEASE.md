# v0.2.8 release handoff and next steps

**Updated:** 2026-07-30
## Current state

The v0.2.8 implementation now includes writer/reader receipts, positioned point
reads, vectored framing, static sync/memory policies, post-write observation,
strict recovery, and reproducible benchmark findings.

Latest completed contributor work:

- **C2:** targeted Miri coverage now includes the positioned-read integration
  binary: caller-scratch borrowing, receipt and offset bounds, one-byte reads,
  and partial-frame retry. Miri isolation cannot open the suite's tempfile-backed
  `File`; that case is explicitly ignored there and remains covered by the native
  gate.
- **A3:** the generic `Read` copy-cost baseline is complete, with the benchmark
  and findings recorded under `docs/benchmark/`; raw snapshots remain local.
- **A4:** compression feasibility — even the highly compressible control became
  slower under LZ4/Zstandard level 1 in the buffered, flush-only file path; no
  production adapter follows.
- **C6:** position-accounting fault semantics — four self-asserting tests
  (`tests/position_accounting_faults.rs`) over a custom `read_vectored` deframer,
  retained bytes before `UnexpectedEof`, device errors counting only returned
  bytes, and start offsets exact across memory reclamation. The later pre-review
  correction removed direct reader/writer access rather than preserving its
  accounting hazards.
- **A2:** position-accounting instruction counts, including DCE-resistant
  writer/read baselines and locally retained pinned raw output.
- **C5:** real-file live-tail retry behavior over separate handles for default
  and CRC-32 framing, clean boundary EOF, and non-EOF device errors.

The core stream remains intentionally headerless. Durable applications own an
external format manifest.

## Before release review

1. Keep the working tree scoped: tests/findings/memory updates belong with the
   task they substantiate.
2. Run `scripts/gate.sh` on the final commit.
3. On the reference machine, run Docker/MSRV, Miri, or fuzz only when the final
   diff changes those boundaries; report exactly what was run.
4. Verify downstream consumers against the final pinned revision and update
   their manifests or migration notes for any incompatible application format
   change. Interval, frame, and byte durability policies remain application
   opt-ins.
5. Peer review should focus on:
   - receipt and byte-position semantics after partial I/O;
   - live-file retry versus finalized recovery;
   - durability failure after frame acceptance;
   - static-policy type composition;
   - benchmark isolation and raw provenance.

## Completed macOS contributor queue

### B3 — post-write observability

The design/example first deliverable was followed by an approved, statically
dispatched `PostWriteObserver`. It reports final success/failure, receipts,
latency, and accepted-but-not-durable outcomes without an OTEL dependency.

### E2 — checksum composition decision memo

Declined with rationale: payload-transforming checksum composition remains a
wire-format decision.

## Completed maintainer/reference-machine queue

- **C2 Miri expansion:** completed 2026-07-30.
- **A3 read-copy cost:** completed 2026-07-29.

## Explicitly deferred or rejected

- No core stream preamble; applications version composition out of band.
- No `IncompleteFrame` error kind; source lifecycle determines whether
  `UnexpectedEof` is transient or a torn finalized tail.
- No transparent compression adapter: A4 found byte savings but worse current
  buffered-file throughput, and any explicit format still requires
  compressed/decompressed bounds and checksum semantics.
- No OTEL runtime dependency in the core crate.
