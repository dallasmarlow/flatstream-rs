# Building on flatstream — Application Onboarding Guide

*Baseline: immutable tag `v0.2.7` (`0b9f486`, 2026-07-24). Audience:
engineers building applications — first up, terminal-output journaling — on
the flatstream library.*

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

The crate is not published to a registry. Pin production applications to the
immutable release tag:

```toml
[dependencies]
flatstream = { git = "https://github.com/dallasmarlow/flatstream-rs", tag = "v0.2.7", features = ["crc32"] }
flatbuffers = "25.9.23"
```

For local application development, use a path dependency instead:

```toml
[dependencies]
flatstream = { path = "../flatstream-rs", features = ["crc32"] }
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
  itself yet (see §8).
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
fsync. At durability points, flush and then call
`writer.get_mut().get_ref().sync_data()` on the underlying file. One writer
per stream — there is no multi-writer coordination, by design.

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
  internal buffer holds its high-water mark, which is the right default for
  steady workloads.

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

## 6. Batching: writing a varying number of events "at once"

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

## 7. The terminal-journaling profile (recommended)

- **Framing:** `ChecksumFramer::new(Crc32::new())` / matching deframer —
  4-byte integrity per frame, right size for small text chunks.
- **Bound:** an explicit small `with_max_frame_len` (e.g. 1 MiB) on every
  reader, including recovery.
- **Schema (app-owned; the framing layer never sees it):**

  ```text
  table TerminalChunk {
      sequence: uint64;             // continuity check during replay
      monotonic_timestamp: uint64;
      channel: uint8;               // stdout | stderr | pty
      data: [ubyte];                // raw text/ANSI bytes, uninterpreted
  }
  ```

- **Reopen:** `recover_file` → truncate only on `TornTail` → resume (§5).
- **Replay:** verify `sequence` continuity; a gap means frames were lost
  *before* the journal (the stream layer cannot lose interior frames
  silently when checksums are on).
- flatstream does not become a raw-text log: the payload stays a FlatBuffer;
  `data:[ubyte]` carries the raw bytes.

## 8. What is NOT in this baseline — plan around it

- **Streams are not yet self-describing.** Writer and reader must agree on
  the composition out-of-band. A **v3 stream header** (magic, version,
  checksum identity — derived from your composition automatically) is the
  very next library slice, and its byte layout is already locked.
  **Treat journal files written against this baseline as disposable
  development artifacts** — durable production journals begin when v3
  lands, and nothing written after that will ever need migration.
- No mmap/borrowed-slice source yet (planned); reads copy each frame once
  from the `Read` source into a reused buffer — payload *access* is
  zero-copy.
- No sealed segments / indexes / footers yet (designed, deferred). No async.
  No fsync scheduling (own your durability points, §3).

## 9. Verifying your integration

The library's verification is local and scriptable — use it:

- `scripts/gate.sh` — fmt, clippy `-D warnings`, full test matrix, rustdoc.
- `scripts/examples.sh` — every example self-asserts; read
  `examples/telemetry_agent.rs` (reference workload, schema-vector batching,
  typed reads), `examples/bounded_adapters_example.rs` (limits and the
  errors they produce), and `examples/sized_checksums_example.rs` (exact
  per-frame overhead, asserted).
- `tests/recovery_tests.rs` — the recovery contract, pinned at every byte
  offset; mirror its reopen pattern in your app tests.
- Wire-format details: `docs/WIRE_FORMAT_SPEC.md` (normative, with a Go
  reference reader).

Write tests that assert outcomes, not printouts — that is the repo's bar,
and your app inherits it.
