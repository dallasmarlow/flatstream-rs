# Design Document: flatstream-rs v2.8 — Writer Frame Receipts and Boundary Ergonomics

**Version:** 1.0
**Status:** Implemented on branch `v0.2.8`; pending review and tag
**Author:** Dallas Marlow
**Date:** 2026-07-24

## 1. Overview

v2.8 is a small, **fully additive, non-breaking** minor release cut in response to
the first real consumer of the library (a terminal scrollback journal built on the
`ONBOARDING.md` §7 profile). It adds three things and changes nothing else — no
wire-format change, no trait-signature change, no behavior change on existing code
paths:

1. **Frame receipts** — writer offset reporting, so external "offset → frame"
   indexes stop reimplementing the wire layout.
2. **`From<flatstream::Error> for std::io::Error`** — the reverse of the existing
   conversion, for application boundaries that normalize on `io::Error`.
3. **`OwnedStreamWriter<W, F>`** — a lifetime-free type alias for the common
   `write_finished`-only writer.

The organizing observation: the consumer succeeded without async, compression,
streamset, or segments, and the only friction was two small additive gaps. The
frame-receipt primitive is also exactly what the v3/segment direction (F1-IMPL's
stream-position accounting, E2's range parser, future segment indexes) needs, so
this release is being pulled by real usage rather than speculative design. `v0.2.7`
stays immutable; `0.3.x+` stays reserved for the roadmap.

## 2. Frame Receipts (writer offset reporting)

The friction, verbatim from the feedback: the consumer tracked
`bytes += 8 + builder.finished_data().len()` alongside every write, coupling its
index to the documented wire layout. v2.8 has the writer report the offset instead.

- **`FrameReceipt { frame_start: u64, wire_len: u64 }`** — the byte position and
  on-wire size of one frame. The next frame begins at `frame_start + wire_len`.
- **`write_with_receipt` / `write_finished_with_receipt`** return a `FrameReceipt`;
  the existing `write` / `write_finished` are unchanged and now delegate to them
  (the discarded receipt's arithmetic dead-code-eliminates, so their hot-path
  codegen is unaffected — see §6).
- **`bytes_written(&self) -> u64`** is the running stream offset (the `frame_start`
  the next write will receive). **`with_start_offset(u64)`** sets the base for a
  writer positioned over a nonzero region of a file, so receipts can carry absolute
  file offsets; it defaults to 0 (stream-relative).

**Implementation.** The writer wraps its underlying `W` in an internal
`CountingWriter<W>` that tallies bytes *actually accepted* by `W`. `wire_len` is the
delta across the framer's write, so it is correct for **any** framer — default,
checksummed, or custom — with **no change to the `Framer` trait** (a trait-signature
change would have been the breaking alternative and was rejected). A mid-frame I/O
error tears the frame; the stream is then recovered/truncated per the E1 recovery
contract, so an unobservable partial count does not affect a well-formed stream.
`get_ref`/`get_mut`/`into_inner` continue to expose `&W`/`&mut W`/`W`.

## 3. `From<flatstream::Error> for std::io::Error`

The crate already had `From<std::io::Error> for Error`; the reverse was missing, so
application code surfacing `io::Error` at its boundaries wrote
`map_err(io::Error::other)` everywhere. The new impl (via `io::Error::other`)
produces an `io::Error` of kind `Other` with the flatstream error preserved as the
inner payload (recoverable through `get_ref`/`into_inner`, and forwarded by
`Display`). Callers that must branch on an original I/O kind match on `Error::kind`
before converting. Cheap and `#[cold]`, consistent with the other conversions.

The alternative — unwrapping the `Io` variant so a round-tripped I/O error keeps
its original kind — was considered and **declined** (2026-07-24): it makes the
conversion's result kind depend on the source variant, and the non-I/O variants
(`ChecksumMismatch`, `ValidationFailed`, `InvalidFrame`) would still be wrapped as
`Other` regardless, so the asymmetry buys little while `other()` is uniform and
discards no context. Revisit only if round-tripping I/O errors through `Error`
proves common in practice.

## 4. `OwnedStreamWriter<W, F>` alias

`StreamWriter`'s `'a` lifetime originates in its internal `FlatBufferBuilder<'a>`
and only bites when the builder borrows external data. `write_finished`-only
consumers never exercise it yet still had to name or infer it (a surprising
`E0106`). `OwnedStreamWriter<W, F> = StreamWriter<'static, W, F, DefaultAllocator>`
pins it away. No second writer implementation was introduced — the alias suffices;
a distinct builder-less type is reconsidered only if the alias proves insufficient.

## 5. Deferred and Rejected

- **Offset-addressed read (`read_frame_at` / `StreamReader::seek_to`) — deferred to
  E2.** A general seek has too many undefined interactions (stateful deframers,
  pending memory-policy shrink, buffered readers, v2-vs-v3 header base) to rush; it
  is the job of E2's range parser. `examples/external_index.rs` demonstrates the
  supported seek-and-fresh-reader pattern in the meantime, and documents the
  per-fetch allocation that E2 will remove with caller-supplied scratch.
- **Fluent builder objects — remain rejected/archived** (`docs/archive/V2_X_FLUENT_BUILDER.md`).
  The consumer report did not ask for a builder; the `FramerExt`/`DeframerExt`
  extension methods already provide the useful, fluent portion.

## 6. Verification

- **Tests:** 42 lib unit tests (incl. new receipt, offset, alias, and
  error-conversion tests) plus the integration and doctest suites, green across
  `all_checksums`, no-features, and `crc16`-only. `cargo fmt --check`, `clippy
  --all-targets -D warnings` (all-features and no-features), and `rustdoc -D
  warnings` all clean.
- **Write-path performance (the consumer's headline concern):** measured against
  the saved post-`v0.2.7` Criterion baseline, the `CountingWriter` + receipt
  routing shows **no regression** — "Sustained Performance: Writing 1000 small
  messages" reports *No change* (simple mode) and *within noise* (expert mode);
  "Checksum Writers" reports *No change* (XXHash64), *within noise* (CRC32), and
  *improved* (CRC16). The offset counter is two integer adds per frame, in the
  noise as expected.
- **Example as executable claim:** `examples/external_index.rs` asserts the index
  tiles the stream contiguously and covers every byte, and that seek-based random
  access returns byte-exact payloads (run by `scripts/examples.sh`).

## 7. Breaking Changes

None. v2.8 is purely additive; every existing constructor, method signature, and
wire byte is unchanged. Version bumped `0.2.7` → `0.2.8`.
