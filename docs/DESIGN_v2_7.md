# Design Document: flatstream-rs v2.7 — Hardening and the Single Read Path

**Version:** 1.0
**Status:** Implemented (uncommitted, pending owner review)
**Author:** Dallas Marlow
**Date:** 2026-07-09

## 1. Overview

This document describes everything that changed between the `v0.2.6` tag ("Rename
crate", #13) and the v0.2.7 release cut. The work landed in two waves:

- **Phase A (merged to `main` via #14–#30):** custom framers, reader ergonomics,
  LOBSTER corpus integration, the validation layer, and adaptive memory policies —
  the library grew its remaining composable strategy surfaces.
- **Phase B (the hardening branch):** a deliberate hardening pass — one
  bounded-by-default read path, a wire-format conformance fix, compile-time checksum
  widths, a pointer-sized error type, manifest hygiene, a scripted local verification
  gate with instruction-count and fuzz coverage, and a determinism seam for time.

The organizing principle for both waves is unchanged from v2.6: zero-copy and
zero-allocation invariants held at all three layers — dispatch (static generics on
the framing/checksum/validation paths in their default configurations; deliberate
opt-in exceptions: `MemoryPolicy` — one boxed call while consulted above its
baseline, measured in a gate-open benchmark at ~1 ns over the no-policy path —
plus `CompositeValidator`, one boxed call per composed
validator, and `TypedValidator`, a function-pointer call, both unmeasured), inlining (`#[inline]`
on thin forwarders, cold paths outlined), and algorithmic residue (no per-frame
zeroing or allocation after the buffer reaches its high-water mark; "zero-copy"
scopes to payload *access* — a generic `Read` source copies each frame once into
the reusable buffer, and the copy-free borrowed-slice source is future work) —
with adopted performance claims measured against saved Criterion baselines and
unmeasured exceptions labeled explicitly.

## 2. Wire Format (unchanged, now specified and enforced)

The frame layout did not change:

```
[4-byte payload length (u32, LE) | checksum (0/2/4/8 bytes, LE, optional) | payload]
```

What changed is its standing: `docs/WIRE_FORMAT_SPEC.md` now defines the format
normatively (framing, checksum truncation, reader state machine, error taxonomy), and
the reader is conformant with §6 in a way v0.2.6 was not:

- **Clean EOF vs. torn frame.** Only zero bytes available at a frame boundary is
  end-of-stream (`Ok(None)`). A partial length header (1–3 bytes then EOF) — silently
  treated as clean EOF in v0.2.6 — is now `ErrorKind::UnexpectedEof`, as is EOF
  anywhere inside a checksum field or payload.
- **Byte-exact checksum widths.** A checksum's on-wire width is its declared `SIZE`,
  never silently widened (the old `_ => 8` fallback in the framer is gone;
  see §5).

## 3. Phase A: Validation Layer (#29)

A composable, opt-in validation layer mirroring the checksum strategy pattern:

- `Validator` trait (`validate(&[u8]) -> Result<()>`, `name()`), `Send + Sync`.
- Implementations: `NoValidator` (zero-cost opt-out), `SizeValidator`,
  `TableRootValidator` (schema-agnostic FlatBuffers table-root verification with
  DoS-limiting verifier options), `TypedValidator` (schema-aware, built from a
  generated root verifier), `CompositeValidator` (AND pipeline, short-circuits).
- `ValidatingFramer`/`ValidatingDeframer` adapters plus `.with_validator(..)` fluent
  constructors. Write-side validation runs before I/O; read-side after deframing
  (and after checksum verification), before the payload reaches application code.
- Failures surface as `ErrorKind::ValidationFailed { validator, reason }`.

## 4. Phase A: Memory Policies (#30) and the Clock Seam (B8)

Long-running writers/readers retain high-water-mark capacity indefinitely; after a
rare large burst, that is memory bloat. The policy layer makes reclamation an opt-in
strategy:

- `MemoryPolicy` trait (`should_reset`, `on_reclaim` hook, `baseline_capacity`),
  installed via `StreamWriter::with_memory_policy` / `StreamReader::with_memory_policy`.
- `AdaptiveWatermarkPolicy` — hysteresis loop over a capacity/message-size ratio with
  optional time cooldown; `SizeThresholdPolicy` — explicit large-event/small-run
  variant; `NoOpPolicy` — benchmark baseline and wrapper filler.
- The machinery is outlined off the hot paths (`#[cold]`/`#[inline(never)]` on the
  reclaim path). Cost with no policy installed is a single predictable branch; the
  baseline gate (consult the policy only while capacity exceeds its baseline) keeps
  steady state at a plain integer compare. Measured per-`write()`: 8.7 ns no policy,
  9.6 ns boxed no-op, 11.1 ns adaptive-installed-not-firing.
- **B8:** time-based triggers now read an injected `Clock` (`fn now(&self) ->
  Duration`, monotonic-since-origin) instead of calling `Instant::now()` directly.
  `AdaptiveWatermarkPolicy<C: Clock = MonotonicClock>` uses a generic default
  parameter, so the seam has zero dispatch cost and every existing `::new()` call
  site compiles unchanged; tests and (eventually) the deterministic simulator inject
  a tick-controlled clock — the policy test suite is now sleep-free. This is the
  first of the determinism seams the DB evolution plan calls for.

## 5. Phase B: One Bounded Read Path (B1, B2)

v0.2.6 shipped four deframers per checksum mode (`Default`, `SafeTake`, `Unsafe`,
`Bounded`-wrapped) because the read path zeroed each frame's buffer region before
reading into it, and the variants existed to dodge that cost. B1 removes the cost
instead of multiplying implementations:

- **Trait redesign.** `Deframer::read_and_deframe` returns `Result<Option<usize>>`
  (payload length; `None` = clean EOF). A provided method reads the 4-byte length and
  delegates to the required `read_after_length`; adapters forward, so bounds and
  callbacks compose on both entry points.
- **High-water-mark buffer.** The reader's buffer grows monotonically and is zeroed
  only on growth (`if n > buf.len() { buf.resize(n, 0) }`); the reader yields
  `&buffer[..n]`. Steady state performs no per-frame zeroing in fully safe code —
  `UnsafeDeframer`, `SafeTakeDeframer`, and their equivalence/edge-case test files
  are deleted, and `BoundedDeframer` with them.
- **Bounded by default.** `DefaultDeframer::new()` / `ChecksumDeframer::new(alg)`
  reject frames declaring more than `DEFAULT_MAX_FRAME_LEN` (16 MiB) with
  `ErrorKind::InvalidFrame` carrying `declared_len` and `limit` — before any
  allocation is sized from attacker-controlled input. The check is one integer
  compare. `with_max_frame_len(..)` tunes it; `unbounded()` is the explicit opt-in
  for trusted streams. (Write-side `BoundedFramer` remains an adapter.)
- **Torn-header conformance with static-size reads.** The spec fix (§2) requires
  distinguishing "zero bytes at a boundary" from "partial header". The obvious
  implementation — one dynamic-length `read()` loop — compiles to a real `memcpy`
  call per frame and measured **+100%** on tight in-memory read loops. The shipped
  implementation probes with a 1-byte `read_exact` (a 1-byte request cannot be torn,
  so its `UnexpectedEof` ⟺ clean boundary) and then `read_exact`s the statically
  sized remainder, which compiles to register loads. `ChecksumDeframer` reads
  `[len | checksum]` as one merged header — the read-side twin of the write-side
  single `write_all`.
- **B2, diagnostic fidelity.** Only `io::ErrorKind::UnexpectedEof` maps to the
  library's `UnexpectedEof`; real I/O errors (e.g. `PermissionDenied` from a failing
  device) propagate intact through both trait entry points, so callers can tell
  "torn frame, truncate and continue" from "device fault, fail stop".

Measured trade-off (Criterion, `pre-b` baseline, tiny 24-byte in-memory frames):
the hardened path costs ~+1.8 ns/frame (~1.3 ns for the torn-header distinction,
~0.26 ns for the mandated bound check, remainder structure). At 4 KiB frames the
paths are at par (74.5 µs vs 76.5 µs per 1000 messages). Write paths improved
(−5…−10% across framers). The revert lever, if the diagnostic is ever judged not
worth 1.3 ns: collapse the probe back to a single `read_exact` header.

## 6. Phase B: Compile-Time Checksum Widths (B4)

`Checksum` now declares its wire width as an associated constant and centralizes
serialization:

```rust
pub trait Checksum {
    const SIZE: usize;                       // wire width, ≤ 8 (enforced at const time)
    fn calculate(&self, payload: &[u8]) -> u64;
    fn verify(&self, expected: u64, payload: &[u8]) -> Result<()>;   // width-masked
    fn write_bytes<'a>(&self, value: u64, out: &'a mut [u8; 8]) -> &'a [u8];
    fn read_bytes(&self, bytes: &[u8]) -> u64;   // LE, low SIZE bytes
}
```

- The runtime width `match` in `ChecksumFramer` (with its silent `_ => 8` widening)
  is gone; width dispatch constant-folds by construction.
- `const { assert!(C::SIZE <= 8) }` guards in the framer/deframer constructors turn
  an invalid custom width into a compile error (requires Rust ≥ 1.79; see §8).
- Default `verify` compares modulo the wire width (`width_mask(SIZE)`): a custom
  `calculate` wider than `SIZE` — legal — verifies against its own truncated wire
  form instead of always failing. For the built-ins the mask folds to identity.
  A 3-byte `Sum24` conformance test pins the byte-exact layout, roundtrip, and
  corruption detection for nonstandard widths.
- Built-in widths: `XxHash64` = 8, `Crc32` = 4, `Crc16` = 2, `NoChecksum` = 0.

## 7. Phase B: Pointer-Sized Errors (B5)

`Error` was a large enum passed by value through every hot-path `Result`. It is
now a pointer-sized newtype over a boxed kind:

```rust
#[derive(thiserror::Error)]
#[error(transparent)]
pub struct Error(Box<ErrorKind>);   // size_of::<Error>() == size_of::<usize>()

#[derive(Debug, thiserror::Error)]
pub enum ErrorKind { Io(..), ChecksumMismatch {..}, InvalidFrame {..},
                     FlatbuffersError(..), ValidationFailed {..}, UnexpectedEof }
```

- `Result<(), Error>` and `Result<usize, Error>` return in registers; the single
  allocation happens on the cold error path, where the error is about to be
  formatted or matched anyway. All constructors and `From` impls are `#[cold]`.
- `thiserror` stays: it derives `Display`/`source` declaratively on `ErrorKind`
  (`#[error(...)]` per variant, `#[from]` on the wrapped `io::Error` and
  `InvalidFlatbuffer`), and the `Error` newtype is `#[error(transparent)]` over
  the box. The size win comes from the `Box`, not from how `Display` is
  implemented — thiserror is build-time only and costs nothing at runtime.
- `InvalidFrame` carries a `&'static str` message plus optional structured context
  (`declared_len`/`buffer_len`/`limit`) rendered on demand at `Display` time via a
  small helper (`InvalidFrameContext`) referenced from the `#[error]` format args —
  no formatting cost at construction.
- Inspection goes through `kind()` / `into_kind()`.
- Size and source-chain tests (`error_is_pointer_sized`, `source_chain_preserved`)
  pin the invariants.
- Measured: read paths unchanged from B1-final (the +52.8%-vs-baseline on tiny
  in-memory frames *is* the documented B1 trade-off, not a B5 regression; B5
  improved on B1-final by ~3%); XXH64 reader −4.9% vs baseline; write paths ≥ par.

## 8. Phase B: Manifest and the Local Verification Gate (B6, B7)

- **Manifest (B6):** unused `tokio` removed (Cargo.lock −192 lines);
  `rust-version = "1.87"` (`is_multiple_of` needs 1.87, inline-const asserts 1.79);
  author email fixed; flatbuffers lock bump to 25.12.19 folded in.
- **Verification gate (B7):** all verification runs locally by deliberate choice —
  no CI spend for a project developed and deployed from owned machines. Four
  scripts under `scripts/` (documented in the README "Verification" section):
  - `gate.sh` — fmt, clippy `--all-targets -D warnings`, the three-combo test
    matrix (all_checksums / no-features / crc16-only), rustdoc `-D warnings`,
    the opt-in `unsafe_typed` integration test, bench compile-checks, and the
    MSRV check when the 1.87 toolchain is installed. Run before every review or tag.
  - `fuzz.sh` — a manually invoked, time-bounded local cargo-fuzz run using the
    Rust nightly toolchain (no CI or scheduler) over the root `fuzz/` workspace:
    `deframe_fuzzer` (arbitrary bytes must never panic or allocate past the bound;
    asserts every yielded payload ≤ `DEFAULT_MAX_FRAME_LEN`) and
    `deframe_checksum_fuzzer` (raw-bytes robustness plus a write→read roundtrip
    asserting byte-identical recovery). The corpus accumulates under `fuzz/corpus/`
    across runs.
  - `instruction_counts.sh` — `benches/instruction_count.rs` (iai-callgrind)
    measures four end-to-end micro-workloads (write/read × default/xxh64) under
    callgrind. Counts avoid wall-clock scheduler/thermal noise but are comparable
    only within one recorded compiler/dependency/target/tool environment; a
    framing-only split is post-release work. Needs valgrind (Linux); the script
    falls back to a versioned Docker image on macOS. Gated behind the
    `instruction_bench` feature so a plain `cargo bench` skips it.
  - `examples.sh` — runs every maintained example, including their executable
    assertions; the LOBSTER ingest example exits cleanly when no corpus is present.
- **Inline audit:** `Messages::{next_message, next}` and
  `TypedMessages::{next_typed, next}` carry `#[inline]` so the iterator facade
  costs nothing cross-crate.

## 9. Phase A Remainder (merged between #14 and #28)

Smaller strands, listed for completeness:

- **Custom framers (#14):** `Framer`/`Deframer` are public, documented extension
  points; `examples/custom_framer_example.rs` maintains a magic-header format.
- **Bounded deframer (#18) and u32 length check (#28):** the fluent-adapter
  ancestors of B1's built-in bound; the write side rejects payloads over `u32::MAX`
  before framing.
- **Reader ergonomics (#21):** `with_capacity`, `reserve`, `buffer_capacity`,
  accessors (`get_ref`/`get_mut`/`deframer`), `into_inner`.
- **LOBSTER corpus (#27):** integration tests and benchmarks against real
  NASDAQ ITCH sample data (feature `lobster`), exercising realistic message-size
  distributions.
- **Benchmark suite structure (#17, #22, #23, #26):** trait-dispatch isolation
  bench, adapter micro-benches, commentary discipline in bench files.

## 10. Breaking Changes

Phase B is API-breaking; backwards compatibility was explicitly out of scope and the
library has no external users yet. This release is **v0.2.7** — strict 0.x SemVer
would call breaking changes a minor bump, but version-number semantics matter only
once there are downstream users, and the 0.3.x+ numbers are already spoken for by
the roadmap's future milestones:

| Change | Break |
|---|---|
| `Deframer` trait signature (`Result<Option<usize>>`, `read_after_length`) | custom deframers must be ported |
| `UnsafeDeframer`, `SafeTakeDeframer`, `BoundedDeframer`, `DeframerExt::bounded` deleted | callers move to `new()`/`with_max_frame_len`/`unbounded()` |
| Deframers bounded by default (16 MiB) | oversized-frame workflows must opt in |
| Torn length header now errors instead of clean EOF | spec conformance; recovery loops see `UnexpectedEof` |
| `Error` → `Error(Box<ErrorKind>)`, variants matched via `kind()` | every `match err { Error::X .. }` site |
| `Checksum::size()` → associated `const SIZE` + `write_bytes`/`read_bytes` | custom checksums |
| `rust-version = 1.87` | older toolchains |

## 11. Verification Summary

- Tests: 121 (all_checksums, incl. doctests) / 95 (no features) / 100 (crc16-only),
  all green; clippy `--all-targets -D warnings` clean; `cargo fmt --check` clean;
  rustdoc `-D warnings` clean. The count is *lower* than mid-pass peaks by design:
  a full test-suite audit before release removed duplicated and vacuous tests and
  strengthened the survivors from count-assertions to byte-exact content
  assertions (record: planning notes, 2026-07-09).
- Wire-format conformance: byte-golden corpus tests against committed frame files
  (the format's bytes, not just its behavior, are pinned); an exhaustive
  truncation sweep asserting the spec §6 reader state machine at every byte
  offset of a multi-frame stream, plain and checksummed; hostile-length,
  HWM-exactness, I/O-error-fidelity, and nonstandard-checksum-width tests.
- Examples double as executable claims: the reference-workload and checksum
  examples assert their own hand-derived expected values (`scripts/examples.sh`
  runs the full suite); a pre-release audit removed examples whose output was
  fabricated or duplicative.
- Criterion baselines: `pre-b` saved before Phase B began; every hot-path change
  measured against it (numbers inline above; logs preserved with the session
  records). Fuzzing and instruction counts run locally via `scripts/` from this
  release on. Not yet verified anywhere: the MSRV 1.87 claim (needs
  `rustup toolchain install 1.87` once) and the Windows build (verify on the next
  Windows machine that builds this).
