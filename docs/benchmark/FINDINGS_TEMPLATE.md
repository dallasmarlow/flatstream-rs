# Findings: <one-line question this experiment answers>

> **How to use this file.** Copy it to `docs/benchmark/FINDINGS_<topic>.md` and
> fill every section in. A findings doc is committed *evidence* — every number
> below must be reproducible from the Methodology section alone, by someone who is
> not you, on a fresh checkout. Delete these `>` guidance blocks as you go.
>
> See `docs/benchmark/BENCHMARK_COMPARISON.md` for a worked example and the
> README's benchmark guidance for the current evidence workflow.

**Author:** <name>
**Date:** <YYYY-MM-DD>
**Status:** in progress | complete

## Hypothesis

> What you suspect, and why — stated so it can be proven *wrong*. If you are not
> risking being wrong, you are not running an experiment.

## Methodology

> The exact, runnable commands and the environment, so the result reproduces.

### Environment

- rustc: `<output of: rustc --version>`
- OS / arch: `<e.g. macOS 15 / arm64, or Linux x86_64>`
- CPU: `<model>`
- Relevant dependency versions: `<e.g. flatbuffers X.Y.Z — from Cargo.lock>`
- Tool: `<criterion X.Y | gungraun X.Y via scripts/instruction_counts.sh>`
- Feature flags: `<e.g. --features all_checksums>`
- Baseline compared against: `<criterion baseline name, or N/A>`

### Steps

```bash
# The exact commands that produced the numbers in Findings. Nothing implicit.
```

> Choose the right instrument and say why: **wall-clock** (Criterion) measures
> throughput but is noisy — report medians *and* the spread, and treat sub-noise
> deltas as no-change. **Instruction counts** (Gungraun/callgrind) are low-noise
> for per-operation deltas but comparable only within one recorded
> compiler/dep/target/tool environment. Do not mix the two into one claim.

## Findings

> The numbers, with the comparison basis stated for each. Tables welcome. Keep
> the gitignored raw Criterion/Gungraun output locally so reviewers can verify
> the summarized values.

## Conclusion

> What it means, and what changes as a result. Be decisive:
> - If a public claim (README, a `DESIGN_v2_x.md`) should change, name it here and
>   change it in the *same* PR.
> - If nothing should change, say that plainly.
> - A **null or negative** result — "the thing I expected to help did not" — is a
>   real result. Commit it. It saves the next person the same experiment.

## Threats to validity

> What could make this wrong or misleading, stated honestly: measurement noise,
> an unrepresentative workload, single-machine / single-run, sink- or
> platform-dependent behavior, a confound you did not isolate. This section is
> where the "measured claims only" bar is actually enforced — an attribution you
> could not isolate belongs here as a caveat, not in Conclusion as a fact.
