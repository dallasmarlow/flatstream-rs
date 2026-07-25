# Raw benchmark output

Committed Criterion output backing the `FINDINGS_*.md` docs. `CONTRIBUTING.md`
§4 and `FINDINGS_TEMPLATE.md` ask findings docs to cite a committed snapshot
rather than a hand-summarized table, and this is where those snapshots live.

Produce them with `scripts/bench_isolated.sh`, which runs **one benchmark group
at a time** and stamps each file with the date, `rustc --version`, host, and the
exact command:

```bash
scripts/bench_isolated.sh e1_file          vectored_framing 'file/'
scripts/bench_isolated.sh e1_tcp           vectored_framing 'tcp/'
scripts/bench_isolated.sh e1_bufwriter     vectored_framing bufwriter
scripts/bench_isolated.sh a1_write_pipeline write_pipeline_decomposition '' -- --features crc32
```

Running the whole suite in one pass is not equivalent. On the development
laptop, unchanged code drifted −24% and +57% between consecutive full runs, and
that drift manufactured a 34% "win" that did not survive isolation
(`FINDINGS_VECTORED_FRAMING.md` threat T1). One group at a time, machine
otherwise idle.

Overwriting a file when re-collecting on different hardware is expected — the
header records which machine produced it, so `git log` carries the history.

## Outstanding

The `FINDINGS_*` docs currently cite four snapshots that have **not been
collected on reference hardware**, which is why both carry
`Status: in progress`:

| Snapshot | Cited by | State |
|---|---|---|
| `a1_write_pipeline.txt` | `FINDINGS_WRITE_PIPELINE_DECOMPOSITION.md` §Findings | not collected |
| `e1_file.txt` | `FINDINGS_VECTORED_FRAMING.md` §Findings | not collected |
| `e1_tcp.txt` | `FINDINGS_VECTORED_FRAMING.md` §Findings | not collected |
| `e1_bufwriter.txt` | `FINDINGS_VECTORED_FRAMING.md` §Findings | header only — the run was interrupted; overwrite it |

The numbers in those docs came from full-suite runs on the development laptop
before the isolation rule existed. They are reported as provisional, and the
direction of every conclusion was re-checked under isolation, but the specific
figures should be replaced by the four commands above before either doc moves to
`Status: complete`.
