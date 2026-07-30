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

## Why this bar

flatstream is not a throwaway utility. It runs in production today under real
load, and it is the foundation for what I am building next — so its quality is not
cosmetic, it is the point. I have put a great deal of myself into it, and I hold it
to a near-perfect standard. I am asking you to hold it there too.

"Near-perfect" is deliberately high, but it is not a mood you have to guess at — it
is *defined*, and you can always tell whether you are meeting it. §1 is that
definition: claims are measured, not asserted; the gate is green before review;
every claim is checked against the source; and you self-review with fresh eyes
before handing work over. Because the bar is objective, it is a target you can hit
rather than a threat.

What I want is care, not fear. Ask anything. Raise uncertainty early — that is
strength here, not weakness — and a benchmark that disproves your own idea is a good
result, not a failure. The bar applies to what we ship; the work of getting there is
allowed to be messy and iterative. Treat the code and docs already in this repo as
the reference for what "done" looks like. I would be glad to have someone who comes
to care about this the way I do.

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
- **Static dispatch by default.** Framing, checksums, sync policy, and memory
  policy are generic and monomorphized. Boxed/dynamic indirection is only ever
  an explicit, opt-in, labeled exception (`CompositeValidator`; typed
  validation uses a function pointer). Measure either before making a cost
  claim.
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
- every maintained example, **executed** (`scripts/examples.sh`) — compiling them
  proves nothing about the assertions §1 requires them to make
- the README's Rust snippets, compiled and run (`scripts/readme_doctests.sh`) —
  rustdoc only tests snippets under `src/`, so this is the one body of example
  code nothing else covers
- `rustdoc -D warnings` (broken intra-doc links are errors)
- bench and fuzz **compile-checks** (targets must not bit-rot)
- an MSRV check of the active toolchain against the `Cargo.toml` floor

Auxiliary scripts (run as appropriate to what you changed):

- `scripts/bench_isolated.sh` — one benchmark group at a time, raw output
  stamped and written to `docs/benchmark/raw/`; see §4
- `scripts/fuzz.sh` — time-bounded cargo-fuzz run (nightly or Docker); corpus
  accumulates under `fuzz/corpus/`
- `scripts/instruction_counts.sh` — Gungraun/callgrind instruction counts
  (valgrind/Linux or Docker), gated behind the `instruction_bench` feature. Use
  this for noise-free per-operation deltas; wall-clock benches are for throughput
- `scripts/miri.sh` — Miri over in-src unit tests and the targeted positioned-read
  integration suite (UB at borrowing, buffer, and offset boundaries)

---

## 4. Benchmarking and the findings format

Running benches is a deliberate, separate act from the compile-check in the gate.
See `docs/benchmark/BENCHMARKING_GUIDE.md` for how to run the Criterion suites and
the baseline flow, and the committed `bench_results*.txt` snapshots for the
recorded record. Criterion baselines are machine-local (gitignored, destroyed by
`cargo clean`); compare against a baseline you saved on the same machine with the
same feature flags, since feature flags change codegen.

**Collect one benchmark group at a time, on an otherwise idle machine.** Use
`scripts/bench_isolated.sh`, which does that and writes stamped raw output to
`docs/benchmark/raw/` for a findings doc to cite. This is not fastidiousness: on
the development laptop, *unchanged* code moved −24% and +57% between consecutive
full-suite runs, and that drift manufactured a 34% "win" that vanished under
isolation (`FINDINGS_VECTORED_FRAMING.md` threat T1). Criterion will call such a
delta statistically significant, because it compares against its own saved
baseline and cannot know the machine rather than the code changed. Two rules
follow:

- Only **A-vs-B pairs collected inside a single isolated run** are admissible.
  Never compare a number from today's run against one written down last week.
- **Re-collect any surprising delta before writing it down.** A result that
  contradicts the mechanism you expected is far more likely to be drift than
  discovery.

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

To start one, **copy `docs/benchmark/FINDINGS_TEMPLATE.md` to
`docs/benchmark/FINDINGS_<topic>.md`** and fill it in — it captures the
environment, the wall-clock-vs-instruction-count distinction, and the
threats-to-validity honesty the bar requires. Findings docs live in
`docs/benchmark/`. Do not update a public performance claim (README, a DESIGN doc)
until a findings doc backs it.

---

## 5. Workflow and review

- Branch off the **active integration branch** (currently `v0.2.8` — ask the
  maintainer if unsure), and open your PR back against that same branch, not
  `main` directly, until it lands on `main`. Keep each branch to one focused
  concern.
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
it touches public API, and follow the definition of done.

**Current assignment order (2026-07-30):** The currently assignable work is
complete. A3 (generic `Read` copy-cost baseline) and C2 (positioned-read Miri
coverage) are recorded below.

The B3/E2 semantic questions are resolved: B3's statically dispatched
post-write hook shipped after maintainer sign-off, E2 is declined, and A4's
benchmark-only compression experiment is complete. Do not start production
compression implementation without maintainer sign-off.

**Contributor environment matters.** The reference-results lane (A3 and extended
C1/C2 runs; A4 used the same lane) requires the maintainer's pinned Docker/Linux
or trustworthy benchmark machine. The macOS-contributor lane (B3 first
deliverable and approved post-write follow-up, E2 decision memo) is complete.

Do not ask a benchmark-incapable contributor to collect or interpret performance
numbers. They may add compile-checked benchmark code for a maintainer to run
only when the task explicitly separates implementation from evidence.

> **Done as of 2026-07-25:** A1, A2, C5, C6, E1, B1 (`tests/external_index.rs` +
> README recipe), B2, C3, C4, D, E3, and E4.
> **Done as of 2026-07-28:** E2 (declined with rationale) and A4 (compression
> feasibility benchmark; no production adapter).
> **Done as of 2026-07-29:** B3 post-write hook after maintainer sign-off, and
> A3 (read-path copy-cost baseline; `FINDINGS_READ_PATH_COPY.md`).
> **Done as of 2026-07-30:** C2 (targeted positioned-read Miri coverage).

### A. Experiments (produce committed findings docs)

**A1 — Write-pipeline decomposition** — **DONE**, `docs/benchmark/FINDINGS_WRITE_PIPELINE_DECOMPOSITION.md`
- **Outcome:** in the isolated 64 B recheck, application harvest/build/index was
  63.0% of the non-durable record, flatstream framing/copy + CRC-32 was 17.3%,
  and buffered file output was 19.7%. At 4 KiB the shares shift to roughly 9%,
  34%, and 57%; attribution is workload-specific. One `sync_data()` per 1,000
  records made the non-sync pipeline 1.4% (64 B) / 16.0% (4 KiB) of elapsed
  time. Raw output and the required 64 B cross-check re-run are committed.
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

**A2 — Position-accounting instruction-count characterization** — **DONE**,
`docs/benchmark/FINDINGS_POSITION_ACCOUNTING.md`
- **Outcome:** discarded writer receipts cost 1.03–2.24 instructions/frame;
  consumed writer receipts cost 5.28–6.10. Reader position accounting costs
  53.91–96.16 instructions/frame, while explicitly consuming each read receipt
  adds only 3.08–4.06. Wall-clock tests resolve no forward-read regression, so
  no API split or optimization follows.
- **Goal:** Nail the per-frame cost of writer receipts and reader position
  tracking across default and checksummed framing.
- **Why:** v0.2.8's wall-clock runs resolve no forward-read regression, but
  counting wrappers and receipt arithmetic execute on every frame. Upgrade that
  result to noise-free counted deltas.
- **Method:** Extend `benches/instruction_count.rs` (run via
  `scripts/instruction_counts.sh`) with paired benchmark-only baselines:
  (a) direct framer write versus `StreamWriter`/receipt accounting, and
  (b) direct deframer loop versus counted `StreamReader`; consume payload bytes
  through `black_box` so LLVM cannot erase the read. Cover default and CRC-32.
- **Deliverable:** findings doc with writer/read deltas per frame and the pinned
  environment fingerprint. Do not infer a production percentage from a mock
  sink.

**A3 — Read-path copy cost** — **DONE** (2026-07-29),
`benches/read_path_copy.rs` + `docs/benchmark/FINDINGS_READ_PATH_COPY.md`
- **Outcome:** the payload copy (`memcpy_only`) is framing-independent and
  bandwidth-bound at ~80 GB/s on the measured M4 Pro. Without a checksum the
  copy is essentially the whole in-memory read (~98% at 4 KiB, ~100% at
  64 KiB+), so a future borrowed-slice/mmap source would reclaim nearly all
  read cost for large un-checksummed frames. With CRC-32 the copy is only ~12–17% at ≥ 4 KiB because
  verification is a second O(payload) pass costing ~7× the memcpy on this build,
  so the source helps checksummed reads far less. No code change, no wire change.
  The CRC-32 `read_copy − borrow_slice` subtraction is catastrophic cancellation
  and unreliable; the copy is reported from `memcpy_only`, cross-validated by the
  default arm (< 4% agreement at ≥ 4 KiB). Runs isolated one framing at a time,
  256 KiB CRC-32 point recollected.
- **Goal:** Quantify the one unavoidable copy (generic `Read` → reader buffer)
  as a fraction of read time across frame sizes.
- **Why:** Establishes a public baseline for the future borrowed-slice/mmap
  source, which is already acknowledged as future work in `docs/DESIGN_v2_7.md`.
- **Design:** three arms per (framing, size) over an identical in-memory wire —
  `read_copy` (real deframe into a reused buffer, the memcpy under test),
  `borrow_slice` (a model of the future borrowed source: same header parse and
  CRC verify, no copy), and a `memcpy_only` cross-check that validates
  `read_copy − borrow_slice` against a raw copy. Default and CRC-32; 64 B–256 KiB.
- **Deliverable:** findings doc; no code change required beyond the bench.

**A4 — Compression feasibility for journal payloads (experiment only)** —
**DONE**, `docs/benchmark/FINDINGS_COMPRESSION_FEASIBILITY.md`
- **Outcome:** modeled schema-exact Palimpsest frames retained 23–55% of current
  wire bytes, but LZ4/Zstandard level 1 made the current buffered, flush-only
  file path 15–44× slower. Even the closest highly-compressible 64 KiB control
  remained 20% / 33% slower on the required recheck. Incompressible data
  expanded. No core adapter, codec dependency, or wire change follows; any
  application-level follow-up needs real anonymized traces plus a raw fallback,
  decoded-size limits, bomb protection, checksum semantics, and a manifest bump.
- **Goal:** Determine whether compression is worth a future explicit format,
  without weakening zero-copy language or adding a runtime dependency first.
- **Method:** In a benchmark-only target, compare uncompressed, LZ4, and a
  low-latency Zstandard level over 4 KiB, 64 KiB, and ~256 KiB payloads.
  Include (a) representative Palimpsest frame payloads, (b) synthetic highly
  compressible data, and (c) incompressible bytes. Measure encode, decode,
  end-to-end buffered-file throughput, and wire-size ratio in paired isolated
  runs. Use caller-owned reusable compression/decompression buffers.
- **Questions the findings must answer:** whether saved write bytes repay
  codec CPU; whether frame-sized latency stays bounded; and how much the
  answer depends on payload distribution.
- **Explicitly out of scope:** production `CompressionFramer`, new wire bytes,
  codec negotiation, or claims of zero-copy decompression. Compression requires
  a decompressed output buffer, compressed/decompressed size limits,
  decompression-bomb protection, and an application manifest entry because the
  core stream is intentionally headerless.
- **Deliverable:** committed benchmark + raw snapshots + findings document.
  A null result or “application-level compression only” is valid.

### B. Polish

> The `io::Error` conversion semantics were revised by owner direction on
> 2026-07-29: preserve an underlying `Io` kind, map flatstream
> `UnexpectedEof` to `io::ErrorKind::UnexpectedEof`, and classify other
> library/protocol failures as `InvalidData`, while retaining the complete
> flatstream error as the inner payload. Rationale in `docs/DESIGN_v2_8.md` §3.

**B1 — External-index recipe** — **DONE**, `tests/external_index.rs` + README
"Frame offsets for external indexing"
- **Outcome:** the integration suite pins contiguity, byte-exactness, `with_start_offset`
  append semantics, checksum-inclusive `wire_len`, and torn-tail survival.
  Direct source/sink access was removed in the pre-review correction round:
  `File` supports I/O through shared references, so removing only `get_mut`
  would not have closed the accounting escape hatch.
- Promote `examples/external_index.rs` into (a) an integration test under
  `tests/` asserting index-contiguity and seek-based random-access correctness,
  and (b) a short README recipe. This is the pattern every index-building
  consumer needs.

**B2 — Post-2.8 documentation consistency sweep** — **DONE** (2026-07-24)
- **Stale versions:** README install snippets said `0.2.7`; `src/lib.rs`'s doc
  header said `v0.2.7`. The lib header is now
  `#![doc = concat!("# FlatStream (v", env!("CARGO_PKG_VERSION"), ")")]`, so it
  cannot drift again. `WIRE_FORMAT_SPEC.md` now states it is verified at v0.2.8
  and unchanged since v0.2.7 — the thing external indexers most need to know.
- **Unmeasured claims:** the stale figures ("84.1% faster", "4.55x", "~8%
  overhead") are all confined to `DESIGN_EVOLUTION.md`, and nothing current
  cites them. That document now opens with a provenance banner marking it a
  historical record and pointing at `docs/benchmark/` for reproducible numbers.
  Its title also claimed "v1 to v2.6" while covering v2.7.
- **Snippets:** all previously `ignore`d rustdoc snippets now compile, and the
  doctest suite has no ignored cases. Several were in `writer.rs` — the module
  whose API changed in 2.8.
- **The real find:** the README's snippets were never compiled by anything, and
  many did not build. ASCII diagrams used untagged fences, which
  rustdoc treats as Rust; one paragraph was indented four spaces and so was also
  parsed as code; the rest were missing imports or `?`-in-`main`. All snippets
  that can run standalone now pass; those requiring generated schema code are
  explicitly `rust,ignore`. `scripts/readme_doctests.sh` enforces the runnable
  set and runs in the gate.

**B3 — Post-write observability boundary** — **DONE** (2026-07-29),
`docs/planning/B3_OBSERVABILITY_BOUNDARY.md` +
`docs/benchmark/FINDINGS_POST_WRITE_OBSERVER.md`
- **Outcome:** `ObserverFramer`/`ObserverDeframer` remain payload inspectors.
  `StreamWriter::with_post_write_observer` installs a concrete,
  statically-dispatched callback after the complete write operation resolves.
  Events distinguish serialization failure, framing/I/O failure, success with
  exact receipt, and durability failure with the accepted receipt. The
  zero-sized default performs no clock read/callback; an installed receipt +
  latency observer costs 31.632 ns/frame in the isolated M4 in-memory harness
  and zero steady-state allocations.
- **Goal:** Give applications one standard, dependency-free pattern for timing
  frame writes, reads, batches, and durability checkpoints, while keeping OTEL
  and metrics crates out of flatstream.
- **Resolve first:** `ObserverFramer` runs before delegated I/O and therefore
  cannot report success, receipt bounds, or latency. Decide whether the correct
  deliverable is only an application recipe/wrapper or a generic post-operation
  hook with explicit success/failure events.
- **Constraints:** no OTEL dependency; no span per frame by default; callback
  cost exists only in the installed concrete type; errors must not be reported
  as successful frames; durability failure occurs after bytes were accepted.
- **Deliverable:** design note, self-asserting example/tests, allocation
  enforcement, and isolated overhead findings. No OTEL/metrics dependency.

### C. Test and robustness hardening

**C1 — Fuzz-corpus growth**
- Run `scripts/fuzz.sh` for extended sessions and commit interesting corpus
  entries under `fuzz/corpus/`. The invariant under test is unchanged: arbitrary
  bytes must never panic or allocate past the configured bound.

**C2 — Miri coverage on read-path boundaries** — **DONE** (2026-07-30),
`scripts/miri.sh` + `tests/positioned_reads.rs`
- **Outcome:** the practical Miri run now covers both the library unit tests and
  the targeted positioned-read integration binary. It executes caller-owned
  scratch reuse and borrowing, exact receipt bounds and offsets, one-byte source
  reads, checksum-width accounting, and same-offset retry after a partial frame.
  The sole excluded case is the retained `BufReader<File>` test: Miri isolation
  forbids the tempfile-backed filesystem boundary, so it is explicitly ignored
  under Miri and remains executed by the ordinary native gate.
- Extend Miri beyond `--lib` so `tests/positioned_reads.rs` exercises
  caller-scratch borrowing, receipt bounds, one-byte reads, and retry after a
  partial frame. Keep the run targeted enough to remain practical.
- Document any integration boundary Miri cannot execute; ordinary gate coverage
  is not a substitute for explicitly stating the gap.

**C3 — Self-assert audit of examples** — **DONE** (2026-07-24)
- **Outcome:** the print-only or under-asserted examples now assert:
  `validation_example` (round-trip equality + write-path rejection leaves the
  sink empty), `adaptive_policy` (records reclamation events and pins the
  hysteresis to message 10), `bounded_adapters_example` (rejected writes leak no
  bytes; over-limit frames never reach the callback), `custom_allocator_example`
  (both write paths round-trip in order), `custom_framer_example` (bad magic,
  torn header, and clean-EOF paths — the example's stated purpose, previously
  unexercised), `multiple_builders_example` (measures builder capacities instead
  of claiming the memory benefit in prose), `ergonomics_example` (message count
  and no-reallocation-after-`reserve`), `ingest_lobster` (`debug_assert` that
  compiled out of release builds, on a counter that incremented even for skipped
  zips).
- **The gap behind the gap:** `scripts/gate.sh` only ever *compiled* examples
  (`clippy --all-targets`), so every assertion in `examples/` was inert in the
  gate. The gate now runs every non-mutating example and derives the list from
  the directory. The corpus-generating `ingest_lobster` is compile-checked by
  default and executes under `RUN_LOBSTER_INGEST=1` when its verified local ZIPs
  are present.
- Audit every `examples/*.rs` for the self-assert rule (§1). Any example whose
  "success" is only a `println!` gets a real assertion or is removed.

**C4 — The zero-allocation steady state, enforced** — **DONE** (2026-07-24),
`tests/allocation.rs`; rationale in `docs/planning/ZERO_ALLOCATION_ENFORCEMENT.md`
- **Why it needed an instrument of its own:** the claim was defended only by
  wall-clock benchmarks, and one allocation per frame costs tens of nanoseconds —
  inside the −24%/+57% drift band §4 documents. A categorical property was being
  guarded by a continuous, noisy measurement.
- **Outcome:** a test-only counting global allocator (`#[global_allocator]` is
  per-binary, so nothing shipped is affected) with a thread-local armed counter.
  The integration suite checks that simple, expert, receipt, checksummed, and
  static-sync-policy write loops plus the read loop
  each allocate and realloc **exactly zero** times in steady state; growth past
  the high-water mark costs, and the frame after it does not; and two harness
  self-tests prove a nonzero result is reachable, so the zero-assertions cannot
  pass vacuously.
- **Sensitivity verified by mutation:** injecting a single `vec![0u8; 8]` into
  `write_all_vectored` failed all three write tests immediately and correctly
  left the read test green. A zero-assertion nobody has seen fail is not yet
  evidence.
- **Scope, stated so it is not oversold:** this pins the *allocation* half of the
  steady-state claim. Copies are a different property — a `memcpy` into an
  already-allocated buffer reports a clean zero here — and remain covered by A3
  and the README's scoping note.

**C5 — Seekable live-file retry hardening** — **DONE**, `tests/live_tail.rs` +
ONBOARDING §6
- **Outcome:** four integration tests on real tempfiles with separate
  append/read handles. The partial-frame retry is parameterized over the framing
  scheme and cuts each frame at several interior points (mid-length-prefix,
  post-header, mid-payload, one-byte-short): each cut reads as `UnexpectedEof`,
  and after the remainder is appended out-of-band the same offset reads
  byte-exact with a receipt naming the whole frame. Default and CRC-32 both
  covered. Clean EOF at the trailing frame boundary is pinned as `Ok(None)`, and
  an injected non-EOF device error (`PermissionDenied`) propagates as `Io`, never
  collapsed into `UnexpectedEof`. No new public error variant; gate green on
  macOS.
- **Goal:** Pin the distinction between “EOF observed now” and “source is
  finalized” without adding an ambiguous `IncompleteFrame` error kind.
- **Method:** Add a real-tempfile test with separate writer/reader handles:
  write part of a default and CRC-32 frame, assert `read_frame_at` returns
  `UnexpectedEof`, append the remainder, then retry the same absolute offset and
  assert byte-exact payload + receipt. Also pin clean EOF at a frame boundary
  and a non-EOF device error.
- **Contract:** the stateless point-read retry is safe because every call seeks
  back to the frame start. Do not imply that a partially consumed sequential
  `StreamReader` can simply continue.
- **Deliverable:** integration tests plus a short live-file tailing recipe in
  ONBOARDING; no new public error variant.

**C6 — Position-accounting fault semantics** — **DONE**,
`tests/position_accounting_faults.rs`
- **Outcome:** four self-asserting tests pin the remaining four cases. (a) A custom
  `read_vectored` deframer (`VectoredDeframer`) yields receipts byte-for-byte
  equal to the writer's on both the sequential and `read_frame_at` paths — on
  both paths `CountingReader` tallies `read_vectored` returns. (b) A mid-frame truncation
  surfaces `UnexpectedEof` while `bytes_consumed` retains every byte the failed
  read consumed — no rollback to the frame start. (c) An injected
  `PermissionDenied` device error propagates intact and adds nothing to the
  counter; only bytes actually returned are counted. (d) A nonzero `with_start_offset`
  composes with an installed `SizeThresholdPolicy`: receipts stay base-relative
  and exact across a buffer reclamation that shrinks the internal buffer
  mid-stream. Mutable reader access was later removed, eliminating the bypass
  cases rather than preserving them as documented hazards.
- **Goal:** Pin what `bytes_consumed` and receipts mean under short reads,
  vectored custom deframers, memory reclamation, and I/O failure.
- **Cases:** (a) a custom deframer that uses `read_vectored` still produces exact
  receipt bounds; (b) successful bytes consumed before `UnexpectedEof` advance
  `bytes_consumed`; (c) a device error counts only bytes actually returned; and
  (d) nonzero `with_start_offset` composes with an installed static memory policy.
- **Constraints:** tests must use deterministic local readers/tempfiles, no
  sleeps, no benchmark claims, and no new public API unless a test exposes an
  unresolvable contract defect.
- **Deliverable:** self-asserting integration/unit tests and any rustdoc
  clarification they prove necessary; `scripts/gate.sh` green on macOS.

### D. Positioned reads — **DONE**

- **Outcome:** `StreamReader` exposes `bytes_consumed`,
  `with_start_offset`, receipt-aware low-level/processor/iterator paths, and a
  stateless `read_frame_at` over `Read + Seek` with caller-owned scratch.
- **Why:** writer receipts removed write-side wire arithmetic, but Palimpsest's
  resume scan still reconstructed checksum/header widths manually and allocated
  a reader buffer for each point lookup.
- **Allocation result:** fresh-reader-per-lookup allocates each frame; warmed
  `read_frame_at` allocates zero times. Forward position tracking has no resolved
  wall-clock regression in the paired benchmark.
- **I/O result:** each point lookup performs one initial seek; a per-call
  `CountingReader` supplies `wire_len` without a post-read position syscall.
  Against the old `stream_position` implementation, bare-file median improved
  9.7% at 4 KiB and 1.3% at 64 KiB on the measured M4. Source buffering remains
  workload-dependent.
- **Live-file boundary:** `UnexpectedEof` describes what the current read
  observed, not whether the file is finalized. A follower retries
  `read_frame_at` with the same absolute offset after more bytes arrive; recovery
  interprets the same condition as a torn tail only after writing has stopped.
- **Evidence:** `tests/positioned_reads.rs`, `tests/allocation.rs`, and
  `docs/benchmark/FINDINGS_POSITIONED_READS.md`.

### E. Carried forward from earlier plans

**E1 — Single-`writev` framing (vectored write)** — **DONE**,
`docs/benchmark/FINDINGS_VECTORED_FRAMING.md`
- **Outcome:** adopted as the default in `DefaultFramer`/`ChecksumFramer`, not
  gated. Every raw-File/TCP pair improved (1.63–3.65× on this machine; one
  surprising arm rechecked at 2.12×). CRC-32/64 B through `BufWriter` repeatedly
  cost ~7% / 1.3 ns more; the default/64 B and both 4 KiB buffered arms were
  inconsistent and are reported as inconclusive. `CountingWriter::write_vectored`
  shipped with it; see the findings doc §F3.
- **Goal:** emit each frame's header (`[len]`, or `[len | checksum]`) and its
  payload in **one `write_vectored` call** instead of two `write_all`s, inside the
  **existing** `DefaultFramer`/`ChecksumFramer` — no new framer types. Two
  `IoSlice`s point at the complete stack header and the borrowed payload in
  both cases: one call, still zero-copy, byte-for-byte identical output (no
  wire change).
- **Why:** on unbuffered sinks (a raw `File`, a `TcpStream`) this halves syscalls
  per frame for high-frequency small writes. It is a call-count / syscall win, not
  a copy win — zero-copy already holds.
- **Correctness — this is the real work:**
  - `Write::write_all_vectored` is **unstable** on the MSRV (1.97.1, issue
    #70436), so hand-roll the partial-write loop over `write_vectored` in one
    small, tested helper. `IoSlice::advance_slices` is **stable** (since 1.81 —
    this task description previously said otherwise) and does the drop/re-slice
    bookkeeping for you. Never assume a single `write_vectored` completes the
    frame.
  - `writev` is **not atomic** across slices — make no all-or-nothing claims.
  - `write_vectored` falls back to sequential writes unless the sink overrides it
    (`File`/`TcpStream` do). `is_write_vectored()` is **unstable** on the MSRV
    (issue #69941 — this task description previously suggested gating on it), so
    a sink's vectoring support cannot be detected. It does not need to be: the
    provided fallback writes the first non-empty slice, the partial-write loop
    picks up the rest, and a non-vectoring sink therefore costs the same two
    calls it did before. `vectored_tests` pins that.
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

**E2 — Checksum framer/deframer inner composition** — **DECLINED** (2026-07-28),
`docs/planning/E2_CHECKSUM_COMPOSITION.md`
- **Outcome:** the backlog's explicitly valid "declined, with rationale" result.
  Every existing adapter is a payload-level pass-through, so wrapping around a
  terminal `ChecksumFramer` (already supported) checksums the identical bytes
  any inner-composition would; a terminal inner would double the length prefix;
  and the one genuinely new capability — checksumming *transformed* bytes —
  changes what the checksum covers, a normative wire-format decision reserved
  for 3.0. Recorded in `docs/DESIGN_v2_8.md` §6; revisit only if 3.0 or an
  approved payload-transforming adapter demonstrates the need.
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

**E3 — Static durability policies** — **DONE**, `src/durability.rs` +
`tests/durability.rs` + `docs/benchmark/FINDINGS_SYNC_POLICY.md`
- **Goal:** make flush-before-sync and group-commit cadence library contracts
  rather than application folklore, while keeping the default write path
  branch-free and statically dispatched.
- **Shape:** `Durable` sinks; zero-sized `NoSync`; frame-, byte-, and
  count-based `SyncPolicy` implementations; static `or` composition;
  automatic and manual `sync_data`/`sync_all`; durable watermarks expressed in
  the same coordinate system as `FrameReceipt::end()`.
- **Failure contract:** a policy checkpoint runs after a complete frame is
  accepted. `DurabilityFailed` records the attempted/previous watermarks and
  triggering frame so callers cannot mistake a sync failure for an unwritten
  frame and duplicate it on retry.
- **Invariants:** no implementation for in-memory sinks; no unsafe or runtime
  dependency; the default state is zero-sized; policy-enabled simple/expert
  loops remain zero-allocation in steady state.
- **Measured outcome:** the default `NoSync` specialization remains the
  zero-overhead baseline. Installing a non-triggering static policy costs
  ~0.281 ns / 30.1 instructions per frame in the mock-sink workloads and zero
  steady-state allocations. Real file elapsed time scales with checkpoint count;
  see the findings for explicit cadences and portability limits.

**E4 — Static memory-policy dispatch** — **DONE**,
`docs/benchmark/FINDINGS_STATIC_MEMORY_POLICY.md`
- **Goal:** remove the per-frame `Option` branch, boxed `MemoryPolicy`, and boxed
  custom-builder factory without weakening writer/reader reclamation semantics.
- **Shape:** zero-sized `NoMemoryPolicy` defaults; concrete
  `WriterMemoryPolicy<P, F>` / `ReaderMemoryPolicy<P>` states; generic builder
  factories; memory and sync policies compose in either installation order.
- **Measured outcome:** the default writer drops 8.21 instructions/frame versus
  the old optional-box carrier. A static gate-open no-op has no resolvable
  writer wall-clock delta and costs 18.14 instructions/frame for the real
  capacity/gate/decision work. Reader rechecks show no regression.

---

## 7. Out of scope

Do not begin, in the course of these tasks: any on-disk/durable format change,
higher-level data constructs, or anything that alters the normative wire bytes.
The stateless `read_frame_at` primitive is the maintainer-approved exception:
it seeks existing v0.2 frames without changing their format. Broader container
or format work remains maintainer-directed.
