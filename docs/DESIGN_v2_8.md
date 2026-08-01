# Design Document: flatstream-rs v2.8 — Receipts, Framing, Durability, and Observability

**Version:** 1.0
**Status:** Implemented on branch `v0.2.8`; pending review and tag
**Author:** Dallas Marlow
**Date:** 2026-07-24
**Updated:** 2026-07-30

## 1. Overview

v2.8 is a pre-1.0 release cut in response to
the first real consumer of the library (a terminal scrollback journal). It
changes no wire bytes. The pre-review correction
round removes mutable source/sink access, makes failed partial writes fail-stop,
and keeps every optional policy/observer layer in a concrete generic writer
type. The vectored path changes sink call shape:

1. **Frame receipts** — writer offset reporting, so external "offset → frame"
   indexes stop reimplementing the wire layout.
2. **`From<flatstream::Error> for std::io::Error`** — the reverse of the existing
   conversion, for application boundaries that normalize on `io::Error`.
3. **`OwnedStreamWriter<W, F>`** — a lifetime-free type alias for the common
   `write_finished`-only writer.
4. **Single-`writev` framing** — the built-in framers emit each frame with one
   vectored write rather than two sequential ones. Output is byte-identical;
   isolated results are recorded in `FINDINGS_VECTORED_FRAMING.md`.
5. **Static durability policies** — `Durable` sinks, automatic frame/byte
   checkpoint policies, manual sync methods, and durable watermarks without a
   branch or trait object in the default writer.
6. **Static memory policies** — writer/reader reclamation state and custom
   builder factories are generic; `NoMemoryPolicy` is the zero-sized default.
7. **Positioned reads** — receipt-aware forward reads, byte-position reporting,
   and retry-safe indexed lookup with caller-owned scratch.
8. **Post-write observation** — a statically dispatched, opt-in callback sees
   final success/failure, exact receipts, elapsed time, and durability failures
   after the frame was accepted.

The organizing observation: the consumer succeeded without async, compression,
streamset, or segments, and the only friction was two small additive gaps. The
frame-receipt primitive is also exactly what segment indexes and positioned
reads need, so
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
  the next write will receive). **`with_start_offset(u64) -> Result<Self>`** sets
  the base before I/O for a writer positioned over a nonzero file region, so
  receipts can carry absolute offsets; it rejects rebasing an active stream.

**Implementation.** The writer wraps its underlying `W` in an internal
`CountingWriter<W>` that tallies bytes *actually accepted* by `W`. `wire_len` is the
delta across the framer's write, so it is correct for **any** framer — default,
checksummed, or custom — with **no change to the `Framer` trait** (a trait-signature
change would have been the breaking alternative and was rejected). A mid-frame
I/O error retains the exact accepted-byte count and poisons the writer. No later
frame or checkpoint is permitted behind the torn frame; callers consume the
sink, recover/truncate it, and reconstruct the writer at the recovered offset.
Only `into_inner` exposes `W`. Direct shared/mutable source access was removed
during the pre-review correction round: types such as `File` permit I/O through
a shared reference, so keeping `get_ref` would preserve the same accounting
escape hatch under a different mutability spelling. Callers consume the stream,
perform raw I/O, and construct a new writer with the resulting absolute offset.

## 3. `From<flatstream::Error> for std::io::Error`

The crate already had `From<std::io::Error> for Error`; the reverse was missing, so
application code surfacing `io::Error` at its boundaries wrote
`map_err(io::Error::other)` everywhere. The conversion preserves the complete
flatstream error as the `io::Error`'s inner payload (recoverable through
`get_ref`/`into_inner`, and forwarded by `Display`) while retaining standard I/O
control flow:

- `ErrorKind::Io(source)` uses `source.kind()` (for example,
  `PermissionDenied`);
- `ErrorKind::UnexpectedEof` uses `io::ErrorKind::UnexpectedEof`;
- checksum, frame, validation, FlatBuffers, and durability failures use
  `io::ErrorKind::InvalidData`.

The original 2026-07-24 implementation normalized every variant to `Other`.
That decision was reversed by owner direction on 2026-07-29: preserving the
diagnostic payload is not enough when an application boundary returns
`io::Result` and must branch on standard kinds. `DurabilityFailed` deliberately
stays `InvalidData` rather than inheriting its source kind because the triggering
frame was already accepted; treating it as an ordinary retryable write error
would violate the durability contract. The conversion remains cheap and
`#[cold]`, consistent with the other conversions.

## 4. `OwnedStreamWriter<W, F>` alias

`StreamWriter`'s `'a` lifetime originates in its internal `FlatBufferBuilder<'a>`
and only bites when the builder borrows external data. `write_finished`-only
consumers never exercise it yet still had to name or infer it (a surprising
`E0106`). `OwnedStreamWriter<W, F> = StreamWriter<'static, W, F, DefaultAllocator>`
pins it away. No second writer implementation was introduced — the alias suffices;
a distinct builder-less type is reconsidered only if the alias proves insufficient.

## 5. Single-`writev` framing

The built-in framers previously put a frame on the wire with two calls:
`DefaultFramer` wrote the length prefix and payload separately, while
`ChecksumFramer` (since v0.2.7) wrote its merged length-plus-checksum header and
then the payload. On a `BufWriter` that is two staging calls; on an unbuffered
sink (raw `File`, `TcpStream`, pipe) it can be two syscalls. Both framers now
build a two-element `IoSlice` array — complete header plus payload — and issue a
single `write_vectored`.

- **The wire format does not change.** The same bytes are emitted in the same
  order; only the call shape differs. The golden hex corpus
  (`tests/wire_format_corpus.rs`, 12 files across four framers) passes unchanged,
  which is what licenses that claim rather than inspection.
- **`Framer` is unchanged.** This is an implementation change inside the two
  built-in framers. Third-party framers are unaffected and keep working; a custom
  framer may adopt the same shape via the shared `write_all_vectored` pattern
  documented in `src/framing.rs`.
- **Partial writes are handled, not assumed away.** `write_vectored` is not atomic
  across slices. The full-acceptance case is peeled out as the hot path; the
  remainder loop lives behind `#[cold] #[inline(never)]`, retries `Interrupted`,
  and converts a stalled sink into `WriteZero` rather than spinning. Covered by
  the `vectored_tests` sinks (one-byte-at-a-time, partial-vectored, stalled).

**Measured** (`docs/benchmark/FINDINGS_VECTORED_FRAMING.md`): every raw-File/TCP
pair improved; the stable isolated range was 1.63–2.12× on this machine. The
production-shaped CRC-32/64 B `BufWriter` pair repeatedly cost
1.29–1.36 ns (~7 %) more; default/64 B and both 4 KiB buffered arms were
inconsistent and are reported as inconclusive. The implementation was adopted
unconditionally rather than gated on a sink probe: a gate would add a branch to
the path it was meant to protect.

**Receipt interaction — why `CountingWriter::write_vectored` ships in the same
release as §2 and §5.** `CountingWriter` tallies scalar and vectored acceptance.
It deliberately relies on the provided `write_all`, which loops through its
counted `write` and therefore retains bytes accepted before a later error.
`Write::write_vectored`'s default also forwards to `self.write`, so the byte
count would remain exact without a vectored override. What that override
actually protects is the §5 win itself: without
it, a framer's single `write_vectored` falls back to two scalar `write`s (header,
then payload via the remainder loop), silently reverting the syscall-halving on
unbuffered sinks — correct bytes, correct offsets, lost vectoring, and no failing
receipt test to notice. `CountingWriter::write_vectored` therefore delegates to
the inner `write_vectored`, and `receipts_are_correct_for_a_vectoring_sink`
asserts the sink genuinely receives one vectored call per frame (the call shape),
not merely that the offsets are right; `tests/external_index.rs` independently
pins receipts against actual frame boundaries.

One follow-up is knowingly left open: `Write::is_write_vectored` is unstable on the
MSRV (`can_vector`, rust#69941), so `CountingWriter` cannot forward it. Nothing in
the current stack queries it — `BufWriter` probes its own inner `File`, not the
`CountingWriter` above it — but an arrangement that nested them differently would
quietly lose the vectored path rather than misbehave.

## 6. Deferred and Rejected

- **Fluent builder objects — remain rejected/archived** (`docs/archive/V2_X_FLUENT_BUILDER.md`).
  The consumer report did not ask for a builder; the `FramerExt`/`DeframerExt`
  extension methods already provide the useful, fluent portion.
- **Checksum framer/deframer inner composition (E2) — declined**
  (`docs/archive/V2_8_E2_CHECKSUM_COMPOSITION.md`). Every payload-level adapter
  already composes around a terminal `ChecksumFramer` with identical checksum
  coverage; the only capability inner-composition would add is a change to what
  the checksum covers, which is a normative wire-format decision reserved for
  3.0. Keep the checksum framers terminal.
- **Observability post-operation hook (B3) — implemented after sign-off**
  (`docs/archive/V2_8_B3_OBSERVABILITY_BOUNDARY.md`). `PostWriteObserver` runs
  after
  the complete writer operation resolves and distinguishes serialization,
  framing/I/O, durability-failure, and exact-receipt success outcomes. The
  zero-sized default performs no timing/callback work; installed overhead is
  recorded in `FINDINGS_POST_WRITE_OBSERVER.md`.

## 7. Verification

- **Tests:** the unit, integration, and doctest suites cover the new receipt,
  offset, alias, and error-conversion behavior across the gate's feature matrix.
  Exact counts are omitted because later test additions make them stale. The
  current gate also runs maintained examples and README snippets and
  compile-checks benchmarks and fuzz targets.
- **Write-path performance:** no cross-run wall-clock claim is made for the
  final correction set. The categorical allocation suite remains the release
  guard; final performance publication requires a same-run A/B recollection.
- **Example as executable claim:** `examples/external_index.rs` asserts the index
  tiles the stream contiguously and covers every byte, and that seek-based random
  access returns byte-exact payloads (run by `scripts/examples.sh`).
- **Non-vectoring sinks do not regress:** `is_write_vectored()` is unstable on the
  MSRV (rust-lang/rust#69941), so a sink that does not implement `writev` cannot
  be detected — and does not need to be. The `vectored_tests` case
  `a_non_vectoring_sink_costs_the_same_two_calls_it_did_before` pins that such a
  sink still sees exactly two calls, with byte-identical output.
- **Durability:** `tests/durability.rs` pins cadence, composition, mode strength,
  `BufWriter` flush ordering, watermarks, start offsets, manual resets, partial
  receipt accounting, and failure context. `tests/allocation.rs` proves
  simple-mode and policy-enabled checkpoint loops allocate zero times in steady
  state. Criterion and pinned instruction counts are recorded in
  `FINDINGS_SYNC_POLICY.md`.
- **Post-write observation:** `tests/post_write_observer.rs` pins success,
  serialization failure, sink failure, and accepted-but-not-durable outcomes.
  `tests/allocation.rs` enforces zero per-frame allocations with an observer
  installed; `FINDINGS_POST_WRITE_OBSERVER.md` records its paired overhead.
- **Memory dispatch:** existing writer/reader reclamation tests pass unchanged,
  including deferred reader shrink and custom factories. The static refactor
  removed the optional box and its dispatch; the committed instruction deltas
  in `FINDINGS_STATIC_MEMORY_POLICY.md` predate final writer fail-stop
  hardening and are marked historical there — recollect before quoting a
  current per-frame figure. Reader reclamation clears the existing
  `Vec` and calls `shrink_to(baseline)` at the deferred boundary rather than
  constructing a second vector. `shrink_to` may still reallocate; reclamation
  remains an intentional cold allocator event, outside the steady-state claim.
- **Positioned reads:** `tests/positioned_reads.rs` pins byte-exact forward and
  random reads, one initial seek per point lookup, start offsets, checksums,
  one-byte source reads, and safe retry after a live file grows. Allocation
  tests prove caller scratch reaches a zero-allocation steady state while fresh
  readers allocate per lookup;
  `FINDINGS_POSITIONED_READS.md` records buffering-dependent wall-clock results.
  Pinned instruction counts in `FINDINGS_POSITION_ACCOUNTING.md` characterize
  discarded/consumed writer receipts and the larger counted-reader boundary;
  no forward wall-clock regression is resolved.
- **Completed verification:** the final local gate and a clean
  `rust:1.97.1-bookworm` gate are green on the exact MSRV; Miri passes all
  in-source unit tests and every in-memory positioned-read integration case.
  Its isolation cannot execute the tempfile-backed `BufReader<File>` case,
  which remains covered by the native gate. Both fuzz targets complete their
  300-second budgets without a crash; every Criterion target and the pinned
  Gungraun suite runs; the corpus-backed LOBSTER integration passes.

## 8. Consuming builders are `#[must_use]`

Methods that take `self` and return a modified `Self` (or a wrapping adapter)
carry `#[must_use]`: `with_memory_policy`, `with_memory_policy_and_factory`,
`with_max_frame_len` on both deframers, `with_cooldown`, `with_baseline`, and the
`FramerExt`/`DeframerExt` combinators `bounded`, `observed`, `with_validator`,
plus `with_sync_policy`, `with_post_write_observer`, and `SyncPolicyExt::or`.

Without it, `deframer.with_max_frame_len(1024);` as a statement compiles clean
and does nothing — the default 2 GiB bound stays in force. That is the failure
mode `#[must_use]` exists for: the knob that stands between a corrupt length
header and a huge allocation, silently not set. The same shape applies to
`bounded()` and `with_validator()`, where a dropped result means the limit or
the validator is simply absent. A `compile_fail` doctest on
`DefaultDeframer::with_max_frame_len` pins the diagnostic.

## 9. Breaking Changes

The wire format and existing constructor signatures are unchanged. The
durability work adds one source-level break permitted by the pre-1.0 policy:
`ErrorKind::DurabilityFailed` is a new public enum variant, so downstream
exhaustive matches must add an arm. `StreamWriter` gains a defaulted sync-state
type parameter; existing type spellings continue to compile because it defaults
to `NoSync`.

The §11 polish round adds `ErrorKind::Poisoned` — the same exhaustive-match
break shape as `DurabilityFailed`. Rejections by a poisoned writer/reader
previously surfaced as `InvalidFrame` distinguishable only by message text; on
this unreleased branch they become the typed `Poisoned` kind (§11.4).

The memory refactor adds defaulted policy-state parameters to `StreamWriter`,
`StreamReader`, `Messages`, and `TypedMessages`. Existing default-state type
spellings continue to compile, but code that explicitly annotated the concrete
return type of `with_memory_policy` or `with_memory_policy_and_factory` must name
the new static policy/factory state.

The `Error` → `io::Error` conversion is additive relative to v0.2.7. Its final
classification preserves underlying I/O kinds and `UnexpectedEof`; other
library/protocol failures become `InvalidData`. The short-lived `Other`
behavior existed only on this unreleased integration branch.

`StreamWriter::{get_ref,get_mut}` and `StreamReader::{get_ref,get_mut}` are
removed. Out-of-band I/O bypassed position accounting and could make every
later receipt wrong (`File` can perform I/O through `&File`, so retaining only
`get_ref` was not sufficient). Callers must use `into_inner`, perform the
operation, and construct a new stream with `with_start_offset` where absolute
receipts are required.

`SyncEveryInterval` remains an explicit opt-in. It reads an injected or
production monotonic clock after each accepted frame so storage-owned time
bounds do not depend on an application scheduler. Applications that already
have a timer may instead call `sync_data`/`sync_all` externally.

`StreamWriter`/`OwnedStreamWriter` gain a defaulted post-write observer type
parameter. Existing type spellings continue to compile; the concrete return
type of `with_post_write_observer` includes the installed observer.

The receipt/position APIs are new in v0.2.8, so their final branch-only
hardening does not break the v0.2.7 surface: `with_start_offset` is fallible and
rejects rebasing after I/O, while `read_frame_at` requires a
`RetrySafeDeframer` so same-offset retries cannot silently reuse stateful decode
state.

The `#[must_use]` additions in §8 can produce **new warnings** in downstream code
that discards a builder's result. Every such warning is a latent bug — the call
was already a no-op — so this is intended, and it is a warning, not an error.

## 10. Durability and boundary follow-ups

> **Status:** §10.1–§10.3 are implemented after maintainer sign-off.

### 10.1 Static durability policies — implemented

**Problem.** A1 measured one `sync_data()` per 1000-record batch at 68.6× the
non-sync 64 B pipeline, but only 5.3× at 4 KiB. The separate cadence benchmark
shows why no cadence-independent multiplier exists. The call is important to a
journaling consumer, but previously the only route was through `get_mut()`, the
one accessor documented to bypass the frame counter.

There is a second, quieter trap. `StreamWriter<BufWriter<File>>` is the
recommended shape, and `writer.get_mut().get_ref().sync_all()` on it syncs
whatever happens to have been flushed — frames still sitting in the `BufWriter`
are not on disk, and nothing says so. "Flush before sync" is knowledge the type
system could carry and currently does not.

**Implemented surface.**

```rust
/// A sink that can push accepted bytes to durable storage.
pub trait Durable: std::io::Write {
    /// Contents only (`fdatasync`); cheaper where metadata need not be current.
    fn sync_data(&mut self) -> std::io::Result<()>;
    /// Contents and metadata (`fsync`).
    fn sync_all(&mut self) -> std::io::Result<()>;
}

impl Durable for std::fs::File { /* delegates */ }
impl<W: Durable + Write> Durable for std::io::BufWriter<W> { /* flush, then sync */ }
impl<W: Durable + ?Sized> Durable for &mut W { /* forward */ }
impl<W: Durable + ?Sized> Durable for Box<W> { /* forward */ }

pub trait SyncPolicy: Send {
    fn observe(&mut self, info: SyncInfo) -> Option<SyncMode>;
    /// `mode` is the strength that actually completed, so a data-only
    /// checkpoint cannot reset a pending metadata requirement.
    fn on_synced(&mut self, mode: SyncMode, durable_watermark: u64) {}
}

let writer = StreamWriter::new(BufWriter::new(file), DefaultFramer)
    .with_sync_policy(SyncEveryNFrames::new(
        std::num::NonZeroU64::new(1000).unwrap(),
        SyncMode::Data,
    ));
```

`NoSync` is the writer's zero-sized default. Installing a policy changes the
writer's sync-state type to `Syncing<P>` and requires `W: Durable`; policy calls
are monomorphized. The default `Vec`/`io::Sink` writer retains its old bounds,
size, and branch-free write path. Built-ins cover every frame, every N frames,
every N wire bytes, and monotonic intervals. `SyncPolicyExt::or` composes
policies statically and chooses `SyncMode::All` when simultaneous decisions
have different strength. Checkpoint completion is also strength-aware:
`SyncMode::All` resets data-only windows, while `SyncMode::Data` cannot reset or
starve a pending metadata checkpoint.

**Checkpoint semantics.** Policies observe a frame only after the sink has
accepted it completely. `BufWriter<W>: Durable` flushes before delegating the
sync. A successful checkpoint stores a durable watermark; every receipt whose
`end()` is at or below it is confirmed by that checkpoint. Manual
`sync_data()`/`sync_all()` return the same watermark and reset the policy window.

If a checkpoint fails, the frame is already accepted. Returning an ordinary
`Io` error would invite a duplicate retry, so `ErrorKind::DurabilityFailed`
records the attempted watermark, previous successful watermark, triggering
frame coordinates (for automatic checkpoints), sync mode, and source error.

**Deliberately not implemented for in-memory sinks.** No `impl Durable for
Vec<u8>` or `io::Sink`. A no-op `sync` that returns `Ok` is a lie of exactly the
kind this crate's culture rejects; absence of the impl makes "this sink has no
durability" a compile-time fact. Test code that needs a durable sink can use a
`tempfile`, as the existing tests already do.

**macOS semantics** *(corrected in the §11 polish round)*. As of Rust 1.97.1,
the standard library issues `fcntl(F_FULLFSYNC)` — a full drive-write-cache
flush — for **both** `File::sync_data` and `File::sync_all` on Apple platforms,
so the two modes are equal in strength there; Linux keeps the
`fdatasync`/`fsync` distinction. An earlier revision of this section claimed
`sync_all` was plain `fsync` with the write-cache caveat, which is wrong for
the MSRV toolchain. The design conclusion stands on the corrected premise:
flatstream delegates to the standard library and adds no platform-specific,
unsafe, or dependency-bearing path — that delegation is simply stronger on
macOS than previously documented.

### 10.2 `FrameReceipt::end()` and `range()` — implemented

**Problem.** `frame_start + wire_len` is written out by hand in the README
recipe, in `examples/external_index.rs`, and in five places in
`tests/external_index.rs`. `FrameReceipt` exists precisely so that consumers do
not do wire arithmetic, and then leaves them doing the one piece that matters —
the piece you must get right to seek to the *next* frame or to bound a read.

**Implemented surface.**

```rust
impl FrameReceipt {
    /// Exact end offset, or None for externally supplied invalid coordinates.
    pub const fn checked_end(&self) -> Option<u64>;
    /// Offset one past the frame's last byte: where the next frame begins.
    pub const fn end(&self) -> u64;
    /// The frame's byte range on the wire, `frame_start..end()`.
    pub const fn range(&self) -> std::ops::Range<u64>;
}
```

All are `const fn`. Flatstream-generated receipts are checked while source/sink
bytes are counted; `end()` is therefore exact for them. For manually constructed
or externally decoded receipt fields, `checked_end()` validates representability
and `end()` saturates rather than panicking or wrapping.

### 10.3 Positioned reads — implemented

**Problem.** §6 deferred this to E2 on the grounds that a general seek has too
many undefined interactions — stateful deframers, a pending memory-policy
shrink, buffered readers, header-base ambiguity. That reasoning holds for
`StreamReader::seek_to`, a *method* that would have to reconcile a seek against
a live reader's buffer and policy state. It does not hold for a **free
function** that owns nothing:

- `Deframer::read_and_deframe(&self, reader, buffer) -> Result<Option<usize>>`
  already takes a caller-supplied `Vec<u8>`. The separate
  `RetrySafeDeframer` marker makes the no-failed-attempt-state requirement
  explicit rather than incorrectly inferring statelessness from `&self`;
  checksum verification composes for the built-ins.
- No `StreamReader` is constructed, so there is no memory policy, no buffer
  ownership question, and no pending shrink.
- Header-base ambiguity is already settled: `FrameReceipt::frame_start` is an
  absolute file offset whenever `with_start_offset` was set, and its rustdoc now
  says so directly rather than "relative to the writer's start offset," which
  read as though the start offset still had to be added.

The old supported pattern allocated a fresh reader and frame buffer per lookup.
The reference consumer also needed forward receipts to retire its remaining
`8 + payload.len()` resume-scan arithmetic.

**Implemented surface.**

```rust
pub struct ReadFrame<'a> {
    pub payload: &'a [u8],
    pub receipt: FrameReceipt,
}

pub fn read_frame_at<'s, R: Read + Seek, D: RetrySafeDeframer>(
    src: &mut R,
    deframer: &D,
    offset: u64,
    scratch: &'s mut Vec<u8>,
) -> Result<Option<ReadFrame<'s>>>;

impl<R: Read, D: Deframer, M: ReaderMemoryBackend> StreamReader<R, D, M> {
    pub fn with_start_offset(self, offset: u64) -> Result<Self>;
    pub fn bytes_consumed(&self) -> u64;
    pub fn read_message_with_receipt(&mut self)
        -> Result<Option<ReadFrame<'_>>>;
}
```

The free function seeks to an absolute offset, decodes exactly one frame through
the supplied deframer, and leaves the source at the next frame boundary. Scratch
grows to a high-water mark and is then allocation-free. Bounds and checksum
semantics remain the deframer's responsibility. After the initial seek it wraps
the source in a per-call counting reader, so the receipt's `wire_len` comes from
bytes actually returned by `Read`; no post-read `stream_position()` syscall is
needed.

Forward `StreamReader` wraps its source in a counting reader. Receipts and
`bytes_consumed()` use the same optional start-offset base as writer receipts.
Neither reader nor writer exposes mutable access to its underlying I/O value;
out-of-band mutation requires consuming and reconstructing the stream.

**Measured result.** Fresh-reader-per-lookup allocates every frame; warmed
caller scratch allocates zero times. Wall-clock direction depends on frame size
and buffering: at 4 KiB a retained `BufReader` narrows the caller-scratch cost,
while at 64 KiB bare and buffered caller-scratch reads slightly outperform the
fresh-reader baseline. See `FINDINGS_POSITIONED_READS.md`.

**Live-file implication.** `UnexpectedEof` means the current read attempt
reached EOF mid-frame, not that the file is finalized. A seekable follower can
retry `read_frame_at` with the same absolute offset after more bytes arrive;
each call rewinds before parsing, and the `RetrySafeDeframer` marker guarantees
the deframer retains no failed-attempt state. Recovery remains the layer that
interprets the same condition as a torn tail after writing has stopped.

## 11. Final refinement and polish round (2026-07-30)

A closing review pass over the complete branch, read/write hot paths first.
It found no correctness, zero-copy, or steady-state-allocation defect;
everything below is a documentation-accuracy correction, additive API surface,
or code hygiene. The wire format and the v0.2.7 constructor surface are
unchanged; §9 records the exhaustive-enum break and the final shape of APIs
introduced on this unreleased branch. The full gate is green on the exact MSRV
after the round.

### 11.1 The macOS durability claim was wrong — corrected everywhere

The library and this document repeated the long-standing caveat that
`File::sync_all` maps to plain `fsync` on macOS and therefore does not flush
the drive's write cache. Checked against the standard-library source shipped
with the MSRV toolchain (Rust 1.97.1,
`library/std/src/sys/fs/unix.rs`): on Apple platforms **both** `sync_all` and
`sync_data` issue `fcntl(F_FULLFSYNC)`. Two consequences:

- Durability on macOS is *stronger* than previously documented, and no unsafe
  platform-specific path is missing — §10.1's delegate-to-std conclusion
  stands on a corrected premise.
- `SyncMode::Data` and `SyncMode::All` are equal in strength on macOS; Linux
  keeps the `fdatasync`/`fsync` distinction.

The `Durable` rustdoc, the README durability FAQ, §10.1 above, and the
`FINDINGS_SYNC_POLICY.md` threats note carried the claim and are corrected
together. The checkpoint costs measured in that findings document were
therefore `F_FULLFSYNC` costs, which strengthens rather than weakens its
"cadence dominates" conclusion. The correction is version-anchored ("as of
Rust 1.97.1") the same way the `can_vector` note is, so a future std change is
a re-verification, not a silent drift.

### 11.2 Observer adapters promoted to crate-root exports

`ObserverFramer`/`ObserverDeframer` were the only composable adapters not
re-exported at the crate root, so naming one (for example, as a config-struct
field type) required a `flatstream::framing::` path no sibling adapter needs.
They are now exported alongside `BoundedFramer` and the validating adapters,
and `examples/observer_adapters_example.rs` imports them the way callers will.
The division of labor with §6's post-write observer is unchanged: payload
inspection before I/O versus final operation outcome after it.

### 11.3 Common-trait derive pass

Strategy and value types now carry the traits a caller needs to hold them in
config structs, hand copies to several writers/readers, key external indexes,
and print diagnostics:

- `DefaultFramer` derives `Debug, Clone, Copy, Default`; `DefaultDeframer`,
  `ChecksumFramer`, `ChecksumDeframer`, `BoundedFramer`, the validating
  adapters, and the observer adapters derive `Debug, Clone, Copy`,
  conditionally on their type parameters (a fn-pointer observer callback
  qualifies; a capturing closure generally is not `Copy`).
- The checksum algorithms and `NoValidator` add `Debug`. `TypedValidator` adds
  `Clone` plus a manual `Debug` printing its registered diagnostic name;
  `CompositeValidator` (boxed inners, underivable) gets a manual `Debug`
  rendering the pipeline by `Validator::name` in evaluation order.
- `FrameReceipt` adds `Hash, PartialOrd, Ord` — receipts are index keys, and
  ordering by `frame_start` (then `wire_len`) is stream order for receipts
  from one stream. `ReclamationInfo` adds `PartialEq, Eq` (matching
  `SyncInfo`); `RecoveryReport` adds `Copy`; `SyncEveryInterval` and
  `AdaptiveWatermarkPolicy` add `Copy` (matching the sibling policies); the
  writer/reader memory-policy state types add conditional `Debug, Clone`; and
  `DefaultBuilderFactory` adds `Debug, Clone, Copy`.

The omissions are deliberate and documented in place. No derived `Default` on
`ChecksumFramer` — construction must flow through `new()` so the const
checksum-width assertion is always evaluated — nor on `ChecksumDeframer`,
where a derived default would zero `max_frame_len`. `PostWriteEvent` and
`PostWriteOutcome` stay `Debug`-only: event types are the most likely to grow
an owned field, and their fields are public and individually copyable, so
locking in `Copy` buys little and costs evolution freedom.

Two self-asserting tests pin the new surface so an added field cannot silently
drop it: `framing::strategy_trait_tests` proves the `Debug + Clone + Copy`
bounds for every strategy type (checksummed variants per feature), and the
validation suite asserts the exact manual `Debug` renderings.

### 11.4 Fail-stop state is inspectable and typed

**Problem.** Fail-stop is a headline behavior of this release (§2, §9), but
the poisoned state was discoverable only by provoking a rejection or wiring a
post-write observer — and the rejection itself surfaced as a generic
`InvalidFrame` distinguishable only by message text.

**Implemented surface.**

```rust
impl StreamWriter { pub fn is_poisoned(&self) -> bool; }
impl StreamReader { pub fn is_poisoned(&self) -> bool; }
ErrorKind::Poisoned // the rejection a poisoned writer/reader returns
```

`is_poisoned()` reports the state without provoking it. Rejections by a
poisoned stream — writes, sequential reads, durability checkpoints, rebasing —
return `ErrorKind::Poisoned`: a state rejection that moved no bytes, distinct
from the fresh frame/protocol failures `InvalidFrame` describes. The
`Error → io::Error` conversion classifies it `InvalidData` with the other
protocol failures (§3). Tests pin both directions of the contract: a failure
that accepted or consumed partial-frame bytes fail-stops writer and reader,
while a zero-byte failure leaves `is_poisoned() == false` and the identical
operation succeeds on retry.

### 11.5 Hygiene

The two byte-identical `NoSync`-state `impl` blocks in `writer.rs` are merged,
and two stale code comments referencing private planning material were
removed. The §11 API additions are additive except `ErrorKind::Poisoned`,
which is recorded in §9.
