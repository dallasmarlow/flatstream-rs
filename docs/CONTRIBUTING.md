# Contributing to flatstream-rs

This is the guide for engineers working **on** flatstream itself. If you are
building an application **with** flatstream, read `ONBOARDING.md` at the repo
root instead — it covers usage patterns and the terminal-journaling profile.

The scope of this document is the work planned **before the 3.0 line**: polish,
hardening, and self-contained experiments that produce reproducible findings.
The 3.0 direction (a durable on-disk format and higher-level data constructs) is
planned and maintainer-directed; it is intentionally **not** detailed here, and
you should not start on it without explicit direction. Everything in the backlog
below is additive to the current 0.2.x line and does not depend on it.

---

## 1. The bar

flatstream holds itself to a deliberately high standard. Two rules matter more
than any style preference:

1. **Measured claims only.** Never write a performance claim you have not
   measured, and never attribute a cost without isolating it. The canonical
   cautionary tale is in the backlog (task A1): a consumer reported a large
   throughput drop and *guessed* it was their own code rather than flatstream —
   plausibly true, but unproven, so it must not be published as fact until a
   decomposition benchmark isolates flatstream's actual share.
2. **The invariants below are contracts, not guidelines.** A change that breaks
   one is wrong even if it compiles and passes tests.

### Invariants

- **Zero-copy payload access.** Readers hand out `&[u8]` borrowed from the
  reader's internal buffer. Adapters introduce no intermediate copy. The one
  unavoidable copy is filling that buffer once per frame from a generic `Read`
  source (see `docs/DESIGN_v2_7.md` §1 and `docs/ZERO_COPY_ANALYSIS.md`).
- **Zero-allocation steady state.** The reader's buffer grows monotonically to a
  high-water mark and is reused; no per-frame allocation or zeroing after that.
  The writer reuses one `FlatBufferBuilder`. Reclamation is an opt-in
  `MemoryPolicy`, off the hot path.
- **Static dispatch by default.** Framing, checksums, and validation are generic
  and monomorphized. Boxed/dynamic indirection is only ever an explicit, opt-in,
  *measured* exception (`MemoryPolicy`, `CompositeValidator`, `TypedValidator`).
  Do not type-erase the framing kernel — see `docs/archive/V2_X_BOXED_TRAITS.md`
  for the standing rejection.
- **`#![forbid(unsafe_code)]` by default.** The crate forbids unsafe in every
  default and feature combination; the sole opt-out is the `unsafe_typed`
  feature. Keep it that way. New unsafe requires a strong, reviewed
  justification and Miri coverage.
- **Wire format is normative and byte-exact.** `docs/WIRE_FORMAT_SPEC.md`
  defines the frame layout, checksum widths, reader state machine, and error
  taxonomy. Consumers build external indexes off it. Changing on-wire bytes is a
  3.0-level decision, not a task here.
- **Examples and tests self-assert.** An example that only prints proves
  nothing; assert the invariant (byte-exact output, roundtrip equality, expected
  error kind). See `examples/external_index.rs` and the `#[test]` modules for the
  style.
- **Breaking changes are acceptable when they improve the design.** There are no
  external API-stability guarantees pre-1.0 and the only current consumers are
  first-party, so prefer additive change when it is free but do not contort the
  API to avoid a break — just record every break in the release doc's
  breaking-change table (see `docs/DESIGN_v2_7.md` §10). Changing the **on-wire
  format bytes** is a different matter: that belongs to the 3.0 format work (§7),
  not because of compatibility but because the format is being redesigned there.
- **Dependencies stay lean.** `thiserror` stays (build-time only). Do not add
  runtime dependencies without a demonstrated need and review sign-off.

---

## 2. Environment

- **Toolchain: Homebrew Rust, no rustup.** Do not write tooling that assumes
  `rustup`. The nightly-only steps (fuzz, Miri) fall back to official Docker
  images when no rustup nightly is present — follow the existing scripts.
- **MSRV is 1.97.1**, declared in `Cargo.toml` (`rust-version`). It tracks the
  production deployment toolchain, not a feature floor. The gate verifies the
  active toolchain satisfies it; when the active `rustc` equals the floor, the
  whole gate has run *on* the MSRV.
- **Exact-MSRV Linux runs** use the pinned `rust:1.97.1-bookworm` container — see
  the README "Verification" / clean-container recipe.

---

## 3. The gate — run before every review, merge, or tag

Verification is **local by deliberate choice; there is no CI.** `scripts/gate.sh`
is the contract. It runs, and a change is not done until it is green:

- `cargo fmt --check`
- `cargo clippy --all-targets -D warnings` across the three-combo feature matrix
  (`all_checksums` full suite incl. doctests / no-features / `crc16`-only), which
  catches `#[cfg]` gaps, plus the opt-in `unsafe_typed` integration test
- `rustdoc -D warnings` (broken intra-doc links are errors)
- bench and fuzz **compile-checks** (targets must not bit-rot)
- an MSRV check of the active toolchain against the `Cargo.toml` floor

Auxiliary scripts (run as appropriate to what you changed):

- `scripts/examples.sh` — runs every maintained example, including its assertions
- `scripts/fuzz.sh` — time-bounded cargo-fuzz run (nightly or Docker); corpus
  accumulates under `fuzz/corpus/`
- `scripts/instruction_counts.sh` — Gungraun/callgrind instruction counts
  (valgrind/Linux or Docker), gated behind the `instruction_bench` feature. Use
  this for noise-free per-operation deltas; wall-clock benches are for throughput
- `scripts/miri.sh` — Miri over the in-src unit tests (UB at buffer boundaries)

---

## 4. Benchmarking and the findings format

Running benches is a deliberate, separate act from the compile-check in the gate.
See `docs/benchmark/BENCHMARKING_GUIDE.md` for how to run the Criterion suites and
the baseline flow, and the committed `bench_results*.txt` snapshots for the
recorded record. Criterion baselines are machine-local (gitignored, destroyed by
`cargo clean`); compare against a baseline you saved on the same machine with the
same feature flags, since feature flags change codegen.

**Experiment deliverables take the form of a committed findings document**, in the
shape of `docs/benchmark/BENCHMARK_COMPARISON.md`:

1. **Hypothesis** — what you suspect and why.
2. **Methodology** — the exact, runnable commands and the environment (rustc,
   deps, target, tool versions). Reproducibility is the point.
3. **Findings** — the numbers, with the comparison basis stated. Distinguish
   wall-clock (throughput, noisy) from instruction counts (deltas, low-noise).
4. **Conclusion** — what it means, and what (if anything) changes as a result.
   If the data is inconclusive, say so; a negative or null result is still a
   result and still gets committed.

Findings docs live in `docs/benchmark/`. Do not update a public performance claim
(README, a DESIGN doc) until a findings doc backs it.

---

## 5. Workflow and review

- Branch off `main`; keep each branch to one focused concern.
- **Changes are independently reviewed before merge**, and the reviewer verifies
  every claim against the code — do not take a summary's word for it. The
  maintainer arbitrates. Write your change so that verification is easy: small
  commits, self-asserting tests, and a findings doc for any measurement.
- **New public API is a design decision.** For anything that adds to the public
  surface (a new method, type, or trait item), agree the shape in review *before*
  implementing, and document it in the relevant `docs/DESIGN_v2_x.md`.
- Release tags and version bumps are maintainer actions. Do not tag.

### Definition of done (every task)

- `scripts/gate.sh` green.
- Any measurement reproducible from a committed findings doc.
- Any new/changed behavior covered by a self-asserting test.
- Docs (rustdoc, README, DESIGN doc) updated to match, with no unmeasured claims.

---

## 6. Pre-3.0 backlog

Each task is self-contained and public-safe. Pick one, confirm scope in review if
it touches public API, and follow the definition of done. Rough priority is A1 →
E1 → B → C → A2/A3 → D/E2, but coordinate with the maintainer. (A1 first because
it tells you where the write cost actually is, which is what decides whether E1's
vectored write is worth shipping.)

### A. Experiments (produce committed findings docs)

**A1 — Write-pipeline decomposition** *(highest value)*
- **Goal:** In a realistic end-to-end write (the terminal-journaling shape is a
  good model), isolate flatstream's actual share of per-record cost from the
  application's.
- **Why:** A consumer observed a large end-to-end throughput drop and attributed
  it to their own serialization/bookkeeping rather than flatstream — plausible
  but unproven. The bar (§1) forbids publishing that attribution unmeasured.
- **Method:** Build a benchmark that measures each stage independently and then
  the full pipeline: (1) harvest/convert only, (2) FlatBuffer building into a
  reused builder, (3) framing into a `Vec`/`io::sink`, (4) CRC32 alone, (5)
  buffered file writes, (6) external-index bookkeeping, (7) full pipeline. The
  existing micro-benches (`benches/benchmarks.rs` Checksum Writers, Deframer
  micro-bench) isolate pieces but not an end-to-end decomposition — build that.
- **Deliverable:** `docs/benchmark/` findings doc answering "what fraction of an
  end-to-end write is flatstream?" with numbers, plus a committed bench file.
- **Note:** phrase CRC32 as hardware-assisted *where SSE4.2/PCLMULQDQ is
  available, scalar fallback otherwise* — not universally accelerated.

**A2 — Frame-receipt instruction-count characterization**
- **Goal:** Nail the per-frame cost of the v0.2.8 frame-receipt path (the
  internal counting writer) across framers, with instruction counts.
- **Why:** v0.2.8 shipped with a wall-clock "no regression" result (see
  `docs/DESIGN_v2_8.md` §6). Upgrade that to a noise-free counted delta.
- **Method:** Extend `benches/instruction_count.rs` (run via
  `scripts/instruction_counts.sh`) to compare `write_finished` vs
  `write_finished_with_receipt` and default vs checksummed framers.
- **Deliverable:** findings doc with the per-frame instruction delta.

**A3 — Read-path copy cost**
- **Goal:** Quantify the one unavoidable copy (generic `Read` → reader buffer)
  as a fraction of read time across frame sizes.
- **Why:** Establishes a public baseline for the future borrowed-slice/mmap
  source, which is already acknowledged as future work in `docs/DESIGN_v2_7.md`.
- **Deliverable:** findings doc; no code change required beyond the bench.

### B. Polish

> The `io::Error` conversion semantics are **settled** (2026-07-24): keep
> `io::Error::other` — uniform kind, no lost context. Rationale in
> `docs/DESIGN_v2_8.md` §3. Not an open task.

**B1 — External-index recipe**
- Promote `examples/external_index.rs` into (a) an integration test under
  `tests/` asserting index-contiguity and seek-based random-access correctness,
  and (b) a short README recipe. This is the pattern every index-building
  consumer needs.

**B2 — Post-2.8 documentation consistency sweep**
- Reconcile README, `docs/DESIGN_v2_8.md`, `docs/DESIGN_EVOLUTION.md`, and
  rustdoc so the writer API surface (receipts, `bytes_written`,
  `with_start_offset`, `OwnedStreamWriter`) is described consistently and with no
  stale or unmeasured claims. Verify every code snippet compiles as a doctest.

### C. Test and robustness hardening

**C1 — Fuzz-corpus growth**
- Run `scripts/fuzz.sh` for extended sessions and commit interesting corpus
  entries under `fuzz/corpus/`. The invariant under test is unchanged: arbitrary
  bytes must never panic or allocate past the configured bound.

**C2 — Miri coverage on read-path boundaries**
- Extend `scripts/miri.sh` coverage over the reader's buffer/offset arithmetic.
  Coverage is expected to grow here; document any boundary the `--lib` run does
  not currently exercise.

**C3 — Self-assert audit of examples**
- Audit every `examples/*.rs` for the self-assert rule (§1). Any example whose
  "success" is only a `println!` gets a real assertion or is removed.

### D. Small feature — reader-side offset reporting *(design sign-off first)*

- **Goal:** Give `StreamReader` the forward-path counterpart to the writer's
  v0.2.8 receipts: a `bytes_consumed()` position accessor and a way to learn the
  byte offset of each frame as it is read (e.g. an offset alongside the payload in
  the `Messages` iterator). This lets a consumer build or verify an external index
  on the read side.
- **Why:** Symmetry with `StreamWriter::bytes_written()` / `FrameReceipt`
  (`docs/DESIGN_v2_8.md` §2), and it is purely additive.
- **Explicitly out of scope:** random access / `seek`-based reads
  (`read_frame_at`, `seek_to`). That work is deliberately deferred and must not
  be pulled forward here — this task instruments only the normal forward read.
- **Process:** confirm the exact API shape in review before implementing (§5),
  then implement with self-asserting tests and a `docs/DESIGN_v2_x.md` note.

### E. Carried forward from earlier plans

**E1 — Single-`writev` framing (vectored write)** *(the maintainer wants this; build + measure)*
- **Goal:** emit each frame's header (`[len]`, or `[len | checksum]`) and its
  payload in **one `write_vectored` call** instead of two `write_all`s, inside the
  **existing** `DefaultFramer`/`ChecksumFramer` — no new framer types. Two
  `IoSlice`s (three with a checksum) point at the stack header bytes and the
  borrowed payload: one call, still zero-copy, byte-for-byte identical output (no
  wire change).
- **Why:** on unbuffered sinks (a raw `File`, a `TcpStream`) this halves syscalls
  per frame for high-frequency small writes. It is a call-count / syscall win, not
  a copy win — zero-copy already holds.
- **Correctness — this is the real work:**
  - `Write::write_all_vectored` and `IoSlice::advance_slices` are **unstable** on
    the MSRV (1.97.1, issue #70436). Hand-roll the partial-write loop over
    `write_vectored` in one small, tested helper (track bytes written; drop
    fully-consumed slices; re-slice the partially-consumed one). Never assume a
    single `write_vectored` completes the frame.
  - `writev` is **not atomic** across slices — make no all-or-nothing claims.
  - `write_vectored` falls back to sequential writes unless the sink overrides it
    (`File`/`TcpStream` do); gate on `is_write_vectored()` where that helps.
  - **Frame-receipt interaction — do not miss this:** the internal `CountingWriter`
    (v0.2.8, `src/writer.rs`) overrides `write`/`write_all`/`flush` but **not**
    `write_vectored`. A framer that starts calling `write_vectored` MUST also get a
    `CountingWriter::write_vectored` that delegates to the inner writer and adds
    the bytes written — otherwise `FrameReceipt`/`bytes_written` silently
    undercount. Prove it with a test.
- **Adopt as the default where it wins.** Breaking changes are fine (§1), so if the
  numbers show a clear win it can become the default framing path rather than a
  gated opt-in; gate only if it helps some sinks and hurts others. Measure first
  regardless (§4): benchmark the vectored path against the current two-call path
  across raw `File`, loopback `TcpStream`, and `BufWriter`, at small and large
  frame sizes, and commit a findings doc. Expect little on `BufWriter` (already
  memcpy-batched on flush); the win is on unbuffered sinks.
- **Deliverable:** the vectored path in the existing framers +
  `CountingWriter::write_vectored` + tests (byte-exact parity with the
  non-vectored path; partial-write loop driven by a one-byte-at-a-time writer) +
  a findings doc.

**E2 — Checksum framer/deframer inner composition** *(smaller; the question is merit, not compat)*
- **Goal:** let `ChecksumFramer`/`ChecksumDeframer` optionally wrap an inner
  `Framer`/`Deframer`, the way `BoundedFramer<F>` / `ObserverFramer<F, C>` already
  do, so a checksum can sit mid-chain rather than only as a terminal. Breaking the
  `ChecksumFramer<C>` signature to add the inner parameter is acceptable (§1) —
  that is not the blocker.
- **Why:** the archived fluent-builder proposal
  (`docs/archive/V2_X_FLUENT_BUILDER.md` §4.2) flagged this as the one remaining
  composability gap.
- **The blocker is semantic — resolve it first:** what does the checksum cover once
  composed? It is defined over the payload, and today bounded/observer already wrap
  *around* a terminal `ChecksumFramer`, so the only genuinely new capability is
  pass-through adapters *inside* it — whose marginal value may be low. Anything
  that changes what the checksum covers is a wire-format decision and belongs to
  the 3.0 work (§7). A documented **"declined, with rationale"** is a valid
  outcome; decide before writing code.

---

## 7. Out of scope

Do not begin, in the course of these tasks: any on-disk/durable format change,
higher-level data constructs, random-access/seek reads, or anything that alters
the normative wire bytes. That work is 3.0-line, maintainer-directed, and tracked
separately. If a backlog task seems to require it, stop and raise it in review.
