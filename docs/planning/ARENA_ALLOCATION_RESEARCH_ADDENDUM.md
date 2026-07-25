# Addendum: `ARENA_ALLOCATION_RESEARCH.md`

**Status:** Commentary — proposes changes to an existing document, implements
nothing
**Date:** 2026-07-24
**Author:** contributor
**Applies to:** `docs/planning/ARENA_ALLOCATION_RESEARCH.md`

This is an addendum rather than an edit because the original is a research record
with real value — in particular it preserves a failed `unsafe` bridge attempt and
prices it, which is exactly the kind of negative result `CONTRIBUTING.md` §4 says
to commit. Nothing below asks to delete any of it. The proposals are: add a
banner, fold in two decisions made elsewhere, and re-scope the open question
using a measurement that did not exist when the document was written.

---

## 1. The document reaches no recommendation, and the decision of record lives somewhere else

The document is explicit that selection is deferred: its Phase 2 is literally
"Solution Selection and Design," it lists four unranked directions, and its footer
calls itself "a living research plan that will be updated as the investigation
progresses." Its conclusion states an opportunity ("By pursuing one or more of the
research directions outlined above…"), not a choice.

Meanwhile `DESIGN_EVOLUTION.md` contains the position the research document lacks:

> "For now, `flatstream-rs` has adopted the pragmatic and safe solution of
> promoting builder reuse as its primary high-performance pattern, which provides
> a significant and risk-free performance benefit."

…and endorses Directions 1 and 2 (upstream contribution; alternative
serialization libraries) as where future research should go.

**Neither document references the other.** A contributor who opens
`docs/planning/ARENA_ALLOCATION_RESEARCH.md` — the obvious place to look — learns
that arena allocation is an open question with four candidate answers, and does
not learn that the project has a standing position, that two of the four
directions are already preferred, or that a third has since been ruled out (§3).

**Proposed:** a header banner on the research document stating the decision of
record and linking `DESIGN_EVOLUTION.md`, in the style the two `docs/archive/`
documents already use.

## 2. The "current implementation" baseline is stale, and nothing says so

Unlike `V3_VECTOR_IO.md` (superseded banner) and the archived docs (rejection
banners), this file carries no staleness marker — yet a long section presents
itself as a factual snapshot of the library and is now wrong:

- It shows `impl<R: Read, D: Deframer> Iterator for StreamReader<R, D>` yielding
  `Result<Vec<u8>>`. That impl no longer exists, and it was removed **precisely
  because it allocated per message** — which makes it a uniquely misleading thing
  to leave standing in a document about allocation.
- It shows the writer holding `writer: W`; the shipped struct holds
  `writer: CountingWriter<W>` (v0.2.8 frame receipts).
- It omits the memory-policy field entirely, so a reader gets no sense that the
  crate already has a memory-management story.

The generic parameter list it shows for `StreamWriter` does still match.

**Proposed:** the same banner as §1 should note the baseline is pre-0.2.6 and
point at `src/` for current shapes.

## 3. One of the four directions has already been decided against

Direction 3 proposes a `SmartBuilder` wrapping `enum BuilderInner { Default(..),
Arena(..), Pooled(..) }` — runtime dispatch over a hot-path strategy.

`docs/archive/V2_X_BOXED_TRAITS.md` was rejected on 2026-07-24 for that exact
shape, and the rejection was written as a general rule rather than a one-off:

> "static by default, measured boxed opt-ins … Existing deliberate indirection
> (`MemoryPolicy`, `CompositeValidator`, `TypedValidator`) is explicitly named,
> opt-in, and measured or documented; it is not precedent for type-erasing the
> framing kernel."

Direction 4 (`AllocationStrategy`, resolved at compile time) is the shape that
rule endorses. Direction 3 is not.

Note also that the document itself concedes Directions 3 and 4 do not solve the
underlying problem — both say verbatim "Still requires solving the underlying
allocator integration." They are packaging, not solutions. That leaves the live
options as **Direction 1 (upstream a better `Allocator` trait)** and **Direction 2
(evaluate a different serialization library)** — which is exactly where
`DESIGN_EVOLUTION.md` already points.

**Proposed:** mark Direction 3 as closed with a reference to the boxed-traits
rejection, and note that 3 and 4 are packaging layers over an unsolved core.

## 4. A1 has priced this work, and the price depends entirely on the durability profile

This is the substantive change I want to propose, and it is only possible because
of a measurement taken after the document was written.

The document sets its own bar: "**Performance**: Achieve at least 15% performance
improvement over current builder reuse approach." When it was written there was no
committed measurement of what builder reuse costs inside a real record, so 15% was
a number without a denominator. The completed
`FINDINGS_WRITE_PIPELINE_DECOMPOSITION.md` (A1) now supplies one.

A1's isolated 64 B recheck measured:

- harvest + FlatBuffer build together are **61.6%** of a non-durable record;
- with one `sync_data()` per 1000-record batch, the *entire non-sync pipeline*
  is **1.4%** of elapsed time.

Carrying the bar through:

| profile | build+harvest share of a record | ceiling on a 15% build-stage win |
|---|---|---|
| `sync_data()` once per 1000-record batch | ~0.88% | **~0.13%** |
| no durability call (network sink, in-memory, `io::sink`) | 61.6% | **~9.2%** |

Two caveats keep this honest. The 61.6% figure combines harvest with build, and
arena allocation only addresses part of the build half, so the measured
batched-sync number is an **upper bound** — the real ceiling is lower. The
figures are specific to the 64 B workload and this machine.

Even as an upper bound, the measured cadence is informative. **With
one `sync_data()` per 1000-record batch, a project that fully meets its own 15%
bar would move the record by about a tenth of one percent on this run** — below
the −24%/+57% noise band `CONTRIBUTING.md` §4 documents. The effect would be
unresolvable on the machine that produced the run.

In a profile with no durability call, the same calculation gives an upper
bound near 9%.

So the arena question is not "is arena allocation worth it." It is **"which
durability profile is `flatstream` optimizing for?"** — and that is a question for
the maintainer, not for a research phase.

**Proposed:** the research document should state its target profile in its
opening, and should not resume Phase 2 until that is settled. If the answer is
the once-per-1000-record cadence A1 modeled, the result argues against resuming
the work without a stronger mechanism. A
different cadence or no durability call is a different profile and needs its
own denominator before this research is accepted or retired.

This is the same reasoning A1 was commissioned to enable. Its stated purpose was
to find out where write cost actually is so that effort could be aimed correctly;
the decision can now be made against the committed A1 evidence.

## 5. A smaller note: the bar should name its instrument

"At least 15% performance improvement" does not say measured how. Given §4, a 15%
win on the build stage in isolation and a 15% win on an end-to-end record are
different claims, and the once-per-1000-record profile leaves little
room for the second. Per `CONTRIBUTING.md` §4, the bar should
name the benchmark, the profile, and whether it is wall-clock or instruction
counts — and given the size of the effect being chased, instruction counts
(A2's instrument) are the only honest choice for the isolated-stage version.

---

## 6. Summary of proposed changes to the original document

1. Add a header banner: decision of record is builder reuse
   (`DESIGN_EVOLUTION.md`); baseline snapshot is stale; see this addendum.
2. Mark Direction 3 closed per the boxed-traits rejection; note that 3 and 4 are
   packaging over an unsolved core, leaving 1 and 2 live.
3. State the target durability profile up front, and gate Phase 2 on the
   maintainer settling it — with A1's numbers (§4) as the input.
4. Restate the 15% bar with a named benchmark, profile, and instrument.

None of this requires touching `src/`.
