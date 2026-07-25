# Building on flatstream — Application Onboarding Guide

*Baseline: `v0.2.8` integration branch (2026-07-25). Audience: engineers
building applications — first up, Palimpsest's terminal-output journal — on
the flatstream library. Pin the reviewed release commit before deployment.*

flatstream is a small, fast framing layer around FlatBuffers for streams
(files, sockets, pipes). It writes and reads sequences of messages as
`[4-byte LE length][optional checksum][FlatBuffer payload]`, preserving
zero-copy access to every payload as a borrowed `&[u8]`. It does not change
how FlatBuffers are encoded, does not manage schemas, and deliberately ships
no daemon, no async runtime, and no network protocol. You compose your
stream format from types; the library stays out of your payload's way.

Start with `README.md` for the full reference; this guide is the shortest
path to a correct journaling application.

---

## 1. Dependency and features

There are **no default features** — checksums are opt-in:

The crate is not published to a registry. During integration, use a sibling
checkout so the application compiles against the exact working branch:

```toml
[dependencies]
# From palimpsest/crates/palim-journal:
flatstream = { path = "../../../flatstream-rs", version = "0.2.8", features = ["crc32"] }
flatbuffers = "25.9.23"
```

After peer review, pin deployment to the immutable release commit rather than
the moving integration branch:

```toml
[dependencies]
flatstream = { git = "https://github.com/dallasmarlow/flatstream-rs", rev = "<reviewed-v0.2.8-commit>", version = "0.2.8", features = ["crc32"] }
flatbuffers = "25.9.23"
```

Available FlatStream features include `"xxhash"` (XXH3-64), `"crc32"`,
`"crc16"`, `"all_checksums"`, and `"unsafe_typed"` (explicitly unsafe
verification-skipping reads). Generate application schemas with a compatible
FlatBuffers 25.x `flatc`.

MSRV is Rust 1.97.1. Breaking changes are allowed between releases — there
are no compatibility shims; read release notes when bumping.

## 2. The five-minute mental model

- **`StreamWriter<W: Write, F: Framer>`** writes framed messages to any
  `Write`. **`StreamReader<R: Read, D: Deframer>`** reads them from any
  `Read` and hands your callback a borrowed `&[u8]` per message — no payload
  copy beyond the one unavoidable `Read`-source copy, no per-message
  allocation once the internal buffer has warmed up.
- **The format is what you compose.** `DefaultFramer` = plain length-prefix.
  `ChecksumFramer::new(Crc32::new())` = length + CRC-32 + payload. Adapters
  wrap any framer/deframer without copying: bounds, observers, validators.
  Writer and reader must be composed to match — the stream does not describe
  itself yet (see §9).
- **Optional policies are static.** `NoSync` and `NoMemoryPolicy` are zero-sized
  defaults that compile away. Installing a sync or memory policy changes the
  concrete writer/reader type and monomorphizes its decisions; there is no
  boxed policy call on the hot path.
- **Two size constants.** Readers accept payloads up to
  `DEFAULT_MAX_FRAME_LEN` (2 GiB — the FlatBuffers maximum buffer size, so
  every valid FlatBuffer works out of the box). `MAX_WIRE_FRAME_LEN`
  (`u32::MAX`, ~4 GiB) is the absolute wire ceiling, reachable only by
  explicit opt-in for raw non-FlatBuffer payloads. Neither limits file size.
  **Always tighten the read bound to your workload** (§4).

## 3. Writing

```rust
use flatstream::{ChecksumFramer, Crc32, StreamWriter, Result};
use std::io::BufWriter;

fn create_new_journal(
    path: &str,
) -> Result<StreamWriter<BufWriter<std::fs::File>, ChecksumFramer<Crc32>>> {
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(path)?;
    let framer = ChecksumFramer::new(Crc32::new());
    Ok(StreamWriter::new(BufWriter::new(file), framer))
}
```

`create_new(true)` deliberately refuses to open an existing journal. Never
wrap an existing read/write file in `StreamWriter` while its cursor is at
offset zero: that would overwrite the journal. Reopen existing files through
the recovery flow in §5; `recover_file` leaves the cursor at the verified
append position before the file is wrapped in `BufWriter`/`StreamWriter`.

Two write styles:

- `writer.write(&value)?` — anything implementing `StreamSerialize`; the
  writer manages an internal `FlatBufferBuilder`.
- `writer.write_finished(&mut builder)?` — you own and reuse the builder
  (`builder.reset()` between messages). Use this on hot paths and whenever
  you build payloads yourself.

**Durability:** `flush()` flushes the `BufWriter` into the OS — it is *not*
fsync. For explicit transaction boundaries, `writer.sync_data()` /
`writer.sync_all()` flush and synchronize a `Durable` sink and return the
durable byte watermark. For automatic group commit, install a static policy:

```rust
use flatstream::{SyncEveryNFrames, SyncMode};
use std::num::NonZeroU64;

let policy =
    SyncEveryNFrames::new(NonZeroU64::new(1_000).unwrap(), SyncMode::Data);
let mut writer = writer.with_sync_policy(policy);
```

The default `NoSync` state is zero-sized and branch-free. A policy-enabled
writer exposes `durable_watermark()`; compare receipts with `receipt.end()` to
learn which frames a successful checkpoint covers. If automatic sync fails,
`DurabilityFailed` reports that the triggering frame was already accepted, so
do not blindly retry the write. One writer per stream — there is no multi-writer
coordination, by design.

**Palimpsest's boundary is a completed harvest, not an arbitrary frame count.**
Its worker writes roughly 256-row frames, flushes once after the whole harvested
RAM range is appended, and only then evicts those rows.

The current `$TMPDIR` scrollback contract is **process-crash recovery**, not a
power-loss-safe WAL. For that contract, keep the default `NoSync` writer and the
existing once-per-harvest `flush()`; it makes indexed frames immediately
readable and the OS page cache normally survives the Palimpsest process.

If the product contract is strengthened, choose the checkpoint deliberately:

- `sync_data()` in `finish_segment()` bounds power-loss exposure to the active
  segment without putting fsync on every harvest.
- `sync_data()` at the end of every `append()` is the stronger promise: no
  harvested row is evicted from RAM before its frame is checkpointed.

```rust
let durable_through = writer.sync_data()?;
debug_assert_eq!(durable_through, writer.bytes_written());
// Under the stronger contract, only now may the engine evict harvested rows.
```

Use `sync_all()` when sealing a segment if file-length/metadata persistence is
part of the deployment contract. Directory-entry and manifest durability
require application-level filesystem handling beyond flatstream. On macOS,
`sync_all()` has `std::fs::File::sync_all` semantics (`fsync`, not
`F_FULLFSYNC`).

## 4. Reading

```rust
use flatstream::{ChecksumDeframer, Crc32, StreamReader, Result};

fn replay(file: std::fs::File) -> Result<u64> {
    const MAX_CHUNK: usize = 1 << 20; // your real ceiling, not the default
    let deframer = ChecksumDeframer::new(Crc32::new()).with_max_frame_len(MAX_CHUNK);
    let mut reader = StreamReader::new(std::io::BufReader::new(file), deframer);

    let mut n = 0;
    reader.process_all(|payload: &[u8]| {
        // payload is borrowed, checksum-verified, zero additional copies
        n += 1;
        Ok(())
    })?;
    Ok(n)
}
```

- `process_all(|&[u8]|)` — drive the whole stream.
- `reader.messages()` — iterator style, early exit friendly.
- `read_message()` — one frame at a time.
- `read_message_with_receipt()` — one frame plus its exact wire range;
  `bytes_consumed()` is the next frame boundary after success.
- `read_frame_at(&mut source, &deframer, offset, &mut scratch)` — stateless
  indexed lookup. Reuse `scratch` across calls for zero-allocation steady state.
- **Typed reads:** implement `StreamDeserialize` for your root type and use
  `reader.process_typed::<T, _>(|root| ...)` — the payload passes your
  schema's verifier before your callback sees the root. (The
  verification-skipping variant exists behind the `unsafe_typed` feature and
  is an explicitly `unsafe fn`; you don't want it until profiling says so.)
- **Validation adapters** run checks at the stream boundary on both paths:
  `framer.with_validator(...)` / `deframer.with_validator(...)` with
  `SizeValidator`, `TableRootValidator` (schema-agnostic structural
  verification), `TypedValidator` (schema-aware), or your own `Validator`.
- Long-running readers/writers can opt into buffer reclamation:
  `.with_memory_policy(AdaptiveWatermarkPolicy::new(..))` — otherwise the
  zero-sized `NoMemoryPolicy` default keeps the high-water mark, which is the
  right choice for Palimpsest's bounded frame sizes and steady replay workload.

## 5. Crash recovery — the contract to build on

A journal that stopped mid-append ends in a torn frame. On every reopen:

```rust
use flatstream::{recover_file, ChecksumDeframer, Crc32, RecoveryEnd, Result};

fn reopen(path: &str) -> Result<std::fs::File> {
    let mut file = std::fs::OpenOptions::new().read(true).write(true).open(path)?;
    let deframer = ChecksumDeframer::new(Crc32::new()).with_max_frame_len(1 << 20);
    let report = recover_file(&mut file, deframer)?;
    if report.end == RecoveryEnd::TornTail {
        file.set_len(report.last_good_offset)?; // drop the torn tail
    }
    // cursor is already at last_good_offset — hand the file to a StreamWriter
    Ok(file)
}
```

The contract is deliberately strict — internalize it:

- `RecoveryEnd::CleanEof` — the file ends on a frame boundary; do nothing.
- `RecoveryEnd::TornTail` — **only** the crash-mid-append signature (partial
  header/checksum/payload, surfaced as `UnexpectedEof`); truncating to
  `last_good_offset` is safe.
- **Everything else is `Err`, never a truncation point**: a checksum
  mismatch is corruption inside a complete frame (or your reader is
  composed with the wrong checksum); `InvalidFrame` can mean a wrong format
  or a too-small bound; `ValidationFailed` can mean validator drift.
  Corruption and misconfiguration never authorize deleting data.
- Run recovery with the deframer that **matches the wire format** — plain,
  matching checksum, *no validators*.
- Scope: append-only journals whose expected failure is a crash during the
  final write. This is not a general repair tool for damaged files.

`recover(reader, deframer)` is the non-seeking variant: it scans from the
reader's current position and reports offsets relative to it.

## 6. Tailing a file another process is still writing

Recovery (§5) is for a journal whose writer has *stopped*. A follower reading a
file that is **still being appended** faces a different, expected condition: it
may reach a frame only partially on disk. `read_frame_at` is the primitive for
this — it is stateless (it seeks to the offset on every call), so a torn read is
never destructive and retrying is always safe:

```rust,no_run
use flatstream::{read_frame_at, ChecksumDeframer, Crc32, ErrorKind, Result};
use std::fs::File;

// Read one frame at `offset`, retrying while the writer is still appending.
// Returns Ok(None) at a clean frame boundary — i.e. "caught up for now".
fn tail_one(file: &mut File, offset: u64, scratch: &mut Vec<u8>) -> Result<Option<u64>> {
    let deframer = ChecksumDeframer::new(Crc32::new()).with_max_frame_len(1 << 20);
    loop {
        match read_frame_at(file, &deframer, offset, scratch) {
            Ok(Some(frame)) => {
                // frame.payload is checksum-verified and borrowed from scratch;
                // frame.receipt.end() is where the next frame begins.
                return Ok(Some(frame.receipt.end()));
            }
            Ok(None) => return Ok(None), // clean boundary: nothing more yet
            Err(e) if matches!(e.kind(), ErrorKind::UnexpectedEof) => {
                // Only part of the frame is on disk. Wait for the appender —
                // e.g. inotify/kqueue, or a bounded sleep — then retry the
                // *same* offset. read_frame_at seeks back, so no state leaks.
                wait_for_more_bytes();
                continue;
            }
            Err(e) => return Err(e), // a real error, not a partial tail
        }
    }
}
# fn wait_for_more_bytes() {}
```

Internalize the boundary the contract draws:

- **`UnexpectedEof` means "this read saw the end of the file mid-frame"** — a
  description of *now*, **not** a claim the file is finalized. On a live file it
  means "the rest hasn't been written yet"; retry the same offset. On a file
  whose writer has stopped, the same condition is a torn tail — which is exactly
  what `recover_file` interprets it as. The distinction is *who is asking*, not a
  different error.
- **`Ok(None)` at a frame boundary means "caught up"**, not "end of file
  forever." A follower loops back and polls the same boundary offset later.
- **Any other `Err` is a real fault** (a device error, a checksum mismatch on a
  complete frame, an oversized length) and must not be retried as if bytes were
  merely missing.
- **Only `read_frame_at` is safe to retry.** A sequential `StreamReader` that
  hit a short read has already consumed the partial bytes through its internal
  counter; it cannot simply continue. Use the stateless point read for tailing,
  or reopen and `recover_file` once the writer has stopped.

The retry, clean-boundary, and device-error contracts are pinned on real files
with separate writer/reader handles in `tests/live_tail.rs`.

## 7. Batching: writing a varying number of events "at once"

- **Recommended — schema-level batching.** One frame carries one FlatBuffer
  whose root holds a *vector* of events (`events:[Event]`, count varies per
  frame). One header + one checksum per burst, zero-copy access to each
  event on read, and **burst atomicity**: the batch is either intact or a
  torn tail that recovery cleanly drops. Note a FlatBuffer has exactly one
  root — a vector inside the root table is the *only* correct multi-event
  frame; you cannot concatenate independent FlatBuffers in one payload.
- **Loop of `write()`/`write_finished()`** — N frames per burst behind a
  `BufWriter`, one `flush()` at the end. Finer-grained recovery: a crash
  mid-burst keeps the intact prefix instead of dropping the whole batch.
- There is **no `write_batch` API** — measured per-message overhead is a few
  nanoseconds, so batching wins come from the schema and the `BufWriter`,
  not from call-count.

Pick by replay semantics: batch-per-flush for all-or-nothing bursts,
frame-per-event for keep-the-prefix recovery.

## 8. The Palimpsest journaling profile

The production consumer lives at `/Users/dallas/repos/palimpsest`, primarily in
`crates/palim-journal/src/lib.rs` and ADR-0013. Its current path dependency
already compiles and all 13 journal tests pass against this branch.

- **Ownership:** one `ScrollbackJournal` per terminal session, owned by the
  single terminal worker. Reads and writes are serialized; the UI never touches
  journal I/O.
- **Capture unit:** when RAM history crosses its threshold, Palimpsest harvests
  the oldest rows and writes them oldest-first. A frame contains a FlatBuffer
  `Frame { first_seq, rows:[Row], mean_ink }`, normally up to 256 rows and a
  256 KiB soft payload target.
- **Framing:** `ChecksumFramer<Crc32>` / matching deframer. Every reader and
  recovery scan uses a 16 MiB hard `with_max_frame_len` ceiling.
- **Segments:** 128 MiB rotation, 16 GiB session budget, whole-segment
  retirement, and a 64-frame decoded LRU.
- **Indexing:** the write path uses `write_finished_with_receipt`; the
  `FrameReceipt::frame_start` is the frame's indexed offset. Use
  `receipt.end()`/`range()` rather than repeating offset arithmetic.
- **Resume:** each segment runs through `recover_file`; only a torn tail on the
  final segment may be truncated. A sequential summary scan rebuilds the
  in-memory index and verifies sequence continuity.
- **Resume scan migration:** use `read_message_with_receipt()` and advance from
  `frame.receipt.end()`. Remove the local `FRAME_WIRE_OVERHEAD` constant and
  `8 + payload.len()` arithmetic.
- **Durability:** `append()` currently flushes once per harvested range,
  intentionally matching process-crash recovery for a temporary scrollback
  cache. If requirements tighten, checkpoint sealed segments first; use
  per-harvest `sync_data()` only when RAM eviction must wait for stable storage.
- **Memory:** frame size and the decoded LRU are already bounded, so the static
  `NoMemoryPolicy` defaults are appropriate unless measurements show a
  high-water-mark problem.

## 9. What is NOT in this baseline — plan around it

- **Streams are intentionally not self-describing.** No core preamble is
  planned; writer and reader composition remains an out-of-band application
  contract. Persistent consumers must carry a manifest/format-generation
  marker and refuse unknown versions before constructing a deframer.
  Palimpsest's segment-set manifest is the reference pattern; bump it on any
  incompatible framing, checksum, or schema change.
- No mmap/borrowed-slice source yet (planned); reads copy each frame once
  from the `Read` source into a reused buffer — payload *access* is
  zero-copy.
- No library-managed sealed segments / indexes / footers yet (designed,
  deferred). No async.
  Durability cadence is local to one writer; there is no cross-stream commit
  coordinator.

## 10. Verifying your integration

The library's verification is local and scriptable — use it:

- `scripts/gate.sh` — fmt, clippy `-D warnings`, full test matrix, rustdoc.
- `scripts/examples.sh` — every example self-asserts; read
  `examples/telemetry_agent.rs` (reference workload, schema-vector batching,
  typed reads), `examples/bounded_adapters_example.rs` (limits and the
  errors they produce), `examples/sized_checksums_example.rs` (exact
  per-frame overhead), and `examples/durability_policy.rs` (automatic/manual
  checkpoints and watermarks).
- `tests/recovery_tests.rs` — the recovery contract, pinned at every byte
  offset; mirror its reopen pattern in your app tests.
- Wire-format details: `docs/WIRE_FORMAT_SPEC.md` (normative, with a Go
  reference reader).

Write tests that assert outcomes, not printouts — that is the repo's bar,
and your app inherits it.

For Palimpsest specifically:

```bash
cd /Users/dallas/repos/palimpsest
cargo test -p palim-journal
```

Run the journal crate plus `palim-term`'s resume/harvest tests after changing
checkpoint semantics; the library gate cannot prove when the application is
safe to evict rows from RAM.
