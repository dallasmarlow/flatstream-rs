# Planning: make the zero-allocation claim a test

**Status:** **Implemented** 2026-07-24 — `tests/allocation.rs`. This document is
kept as the rationale and records the C4 outcome.
The reader implementation later changed from replacing its `Vec` to deferred
`clear` + `shrink_to`; the proposal's Item 6 analysis is historical and no
longer describes a guaranteed reallocation.
**Date:** 2026-07-24
**Author:** contributor
**Targets:** pre-3.0. Landed as **C4** test and robustness hardening.
**Wire format:** unchanged. **Public API:** unchanged — this is test-only
infrastructure.

---

## 1. The gap

The crate's headline property is stated everywhere: zero-copy payload access, and
a **zero-allocation steady state** once buffers reach their high-water mark. It is
argued in `DESIGN_EVOLUTION.md`, scoped honestly in the README's TL;DR, and
demonstrated in the benchmarks.

Nothing fails when it breaks.

That is the whole problem. The claim is currently defended by wall-clock
benchmarks, and a wall-clock benchmark is the wrong instrument for it. A single
small allocation per frame costs on the order of tens of nanoseconds —
comfortably inside the measured **−24% to +57%** noise band between full-suite
runs on unchanged code. A regression that adds one `Vec`
allocation to the hot path would not merely be hard to see; it would be
indistinguishable from the machine having a bad afternoon.

Meanwhile the failure is easy to introduce and looks innocent in review: a
`format!` in an error path that turns out to be on the hot path, a `.to_vec()`
where a slice would do, a `Box::new` per frame in a new adapter, a `collect()` in
a convenience method. Each is a one-line change that no existing check rejects.

This is a categorical property — the count is zero or it is not — being guarded by
a continuous, noisy instrument. Count it instead.

## 2. What is proposed

A test-only counting global allocator and a small harness, in a new integration
test, that arms a counter around a warmed steady-state loop and asserts the count
is exactly zero.

```rust
// tests/allocation.rs — sketch, not final
#[global_allocator]
static COUNTER: CountingAllocator = CountingAllocator;

/// Runs `body` with allocation counting armed. Returns the counts.
fn measure<T>(body: impl FnOnce() -> T) -> (Counts, T);

#[test]
fn steady_state_write_allocates_nothing() {
    let mut writer = /* ... warmed to high-water mark ... */;
    let (counts, _) = measure(|| {
        for _ in 0..1000 { writer.write_finished(&mut builder).unwrap(); }
    });
    assert_eq!(counts.allocs, 0);
    assert_eq!(counts.reallocs, 0);
}
```

`#[global_allocator]` is per-binary, and each integration test is its own binary,
so this affects nothing the library ships and nothing another test observes.

### Mechanism notes

The details below are the ones that decide whether this works at all; they are
listed because each has a way of going wrong silently.

- **Counting must be thread-local.** `cargo test` runs tests in parallel threads
  in one binary, so a global `AtomicUsize` would attribute one test's allocations
  to another's measurement window. The allocator runs on the allocating thread,
  so a `thread_local!` counter attributes correctly and needs no synchronization.
- **The thread-local must be `const`-initialized** (`const { Cell::new(0) }`).
  Lazy TLS initialization can itself allocate, which would recurse into the
  allocator being measured.
- **Arm and disarm explicitly.** Setup — building the writer, warming the builder,
  reserving the sink — allocates by design. Only the region inside `measure` is
  counted.
- **Everything that formats must sit outside the armed region.** `format!` and
  assertion machinery may allocate. `Instant::now()` does not allocate, but it
  belongs inside the region only when the test intentionally covers an
  observer or interval policy. Collect counts first, assert after.
- **Warm to the true high-water mark.** The steady state is only reached once the
  builder and the reader's buffer have grown to the largest payload the loop will
  use. A warmup that uses smaller payloads than the measured loop will show
  allocations that are correct behavior, not a regression.
- **The sink is part of the contract under test.** A `Vec<u8>` sink grows and
  therefore allocates; that is the sink's behavior, not the library's. Use
  `io::sink()`, or a `Vec` pre-reserved beyond the loop's total and asserted not
  to have grown.
- **Run across the feature matrix.** Checksums change the write path; the gate
  already runs `all_checksums` / no-features / `crc16`-only, and this test should
  follow.

## 3. What to pin

The first four are the claim itself. The last two are the more interesting half:
they pin behavior that is currently *folklore*, and turn two of the subtleties
found in the source audit into asserted facts.

| # | Property | Why |
|---|---|---|
| 1 | `write_finished` steady state → 0 allocs | Expert mode is the documented high-throughput path |
| 2 | `write` (simple mode) steady state → 0 allocs | Simple mode is what most users start with, and it owns its builder |
| 3 | `read_message` steady state → 0 allocs | The read path's whole design is a reused buffer |
| 4 | `write_finished_with_receipt` steady state → 0 allocs | New in 0.2.8, and `CountingWriter` sits in the path |
| 5 | The memory-policy reclaim path allocates **exactly when the policy fires** | Converts "the policy trades allocation for RSS" from prose into a measurement |
| 6 | After a policy shrink, the next read re-allocates | Documents a real, currently-unwritten consequence — see below |

Item 6 deserves its own note. `read_payload` gates on `buffer.len()`, not
`capacity()`, so the high-water mark is the vector's *length*; and
`apply_pending_shrink` installs a fresh `Vec::with_capacity(baseline)` whose
length is 0. The next read therefore resizes and re-zeroes, even though the
`Deframer` trait doc says the buffer is a high-water mark that implementations
"never shrink … so steady-state reads touch memory exactly once." Both statements
are true of their own scope, and the interaction between them is written down
nowhere. A test is the right place to fix that: it makes the behavior visible,
pins it against accidental change, and gives the maintainer a concrete artifact to
decide against if the behavior turns out to be unwanted.

Item 5 is what makes this more than a regression guard. The adaptive policy's
entire value proposition is a trade — give up some allocations to give back RSS —
and today the giving-up half is unquantified.

## 4. Why this instrument, next to the ones already planned

The backlog already has **A2** (frame-receipt instruction counts) and **A3**
(read-path copy cost). This is not a substitute for either; the three catch
different things, and it is worth being explicit about which is which.

| Instrument | Catches | Noise | Portability |
|---|---|---|---|
| Criterion wall-clock | throughput regressions | high (±20%+, documented) | everywhere |
| Gungraun instruction counts (A2) | small per-op cost drift | very low | valgrind/Linux or Docker |
| **Allocation counting (this)** | **categorical breaks** | **none — it is a count** | **everywhere, in the gate** |

Instruction counts are the sharper instrument for "did this get 3% more
expensive," and they are the right tool for A2. But they need valgrind, which
means Linux or Docker, which means they will not run in the gate on the
maintainer's machine — `scripts/instruction_counts.sh` is explicitly an auxiliary
script. Allocation counting is pure Rust, runs anywhere, costs milliseconds, and
so can live in the gate and guard the property on **every** run.

The two are complementary in kind, not just in cost: an instruction count tells
you something got slower, and an allocation count tells you something became a
different category of thing.

## 5. What this does **not** prove

Worth stating plainly, because the crate's culture is to scope claims precisely
and this one is easy to oversell.

**It does not prove zero-copy.** Copies and allocations are different properties.
A `memcpy` into an already-allocated buffer allocates nothing and this harness
would report a clean zero. The README's honest scoping already says a generic
`Read` copies each frame once into a reusable buffer; that copy is by design and
invisible here.

So this pins the **allocation** half of the steady-state claim, and only that
half. The copy half is harder to assert mechanically and is better served by A3's
measurement plus the existing prose scoping. Any findings doc arising from this
work should say so rather than let the reader generalize.

**It does not prove absence of allocation on error paths**, only on the measured
happy path. Error construction is allowed to allocate — `Error`'s payload lives
behind a `Box` by deliberate design, so that the hot path stays pointer-sized.

## 6. Deliverable

- `tests/allocation.rs`: the counting allocator, the `measure` harness, and the
  six pinned properties in §3.
- A short findings doc **only if** the memory-policy measurements in items 5 and 6
  produce a number worth publishing. The four zero-assertions are a test, not an
  experiment, and do not need one.
- A note in the README or `DESIGN_EVOLUTION.md` changing "zero-allocation steady
  state" from an assertion into a reference to the self-asserting test that
  proves it.

## 6a. What implementation actually found

Recorded because a proposal that predicts an outcome should say whether it was
right.

**The steady state was already clean.** All four zero-assertions passed on the
first run — no regression was lurking. That is the good outcome, and it means the
value of this work is prospective: it holds a property that currently holds.

**E3 follow-up:** the implemented suite now has nine tests, including six
steady-state zero assertions: simple, expert, receipt, checksummed, static
sync-policy write loops and the read loop. The policy-enabled test contains a
successful checkpoint inside the armed region.

**Two design points changed during implementation.** `realloc` needed its own
counter, because `Vec` growth past capacity reallocs rather than allocs and a
harness watching only `alloc` would miss the most likely regression shape. And
the sink had to be a fixed-capacity writer that asserts its own bound: a `Vec<u8>`
sink grows and would charge its reallocs to the library, while `io::sink()`
discards and exercises no real write path.

**The harness self-tests turned out to be the important part.** A zero-assertion
is only evidence if a nonzero result is reachable, so two tests prove the counter
observes allocations and that arming/disarming works. Beyond those, sensitivity
was confirmed by mutation: injecting one `vec![0u8; 8]` into `write_all_vectored`
failed all three write tests and correctly left the read test green. Without that
step the suite would have been seven tests nobody had ever seen fail.

**Item 6 from §3 was not implemented as specified.** The post-shrink re-allocation
behavior needs a `StreamReader` driven through a memory policy far enough to fire
a shrink, which is a longer fixture than the rest of the file and pins behavior
the maintainer may want to *change* rather than record. It is left for a
follow-up, together with item 5's policy measurement — the two that would produce
a findings doc rather than a plain test.

## 7. Estimated shape

Small. The allocator and harness are perhaps 60 lines; each pinned property is a
short test. The work is almost entirely in getting the warmup and the armed region
right, which is why §2's mechanism notes are the substance of this proposal rather
than an appendix.

The risk of the change is close to zero — it adds a test file and touches no
library code — which is unusual for something that converts the crate's central
performance claim from prose into an enforced invariant.
