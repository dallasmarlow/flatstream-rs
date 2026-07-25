# Raw benchmark output

Committed Criterion output backing the `FINDINGS_*.md` docs. `CONTRIBUTING.md`
§4 and `FINDINGS_TEMPLATE.md` ask findings docs to cite a committed snapshot
rather than a hand-summarized table, and this is where those snapshots live.

Produce them with `scripts/bench_isolated.sh`, which runs **one benchmark group
at a time** and stamps each file with the date, `rustc --version`, host, and the
exact command:

```bash
scripts/bench_isolated.sh e1_file          vectored_framing 'file/' -- --features crc32 --locked
scripts/bench_isolated.sh e1_tcp           vectored_framing 'tcp/' -- --features crc32 --locked
scripts/bench_isolated.sh e1_bufwriter     vectored_framing bufwriter -- --features crc32 --locked
scripts/bench_isolated.sh a1_write_pipeline write_pipeline_decomposition Decomposition -- --features crc32 --locked
scripts/bench_isolated.sh e3_sync_dispatch sync_policy Dispatch -- --locked
scripts/bench_isolated.sh e3_sync_file     sync_policy Cadence -- --locked
scripts/bench_isolated.sh a1_durability    write_pipeline_decomposition Durability -- --features crc32 --locked
scripts/bench_isolated.sh e4_memory_writer memory_policy_benchmarks policy_overhead -- --locked
scripts/bench_isolated.sh e4_memory_reader_recheck memory_policy_benchmarks reader_policy_overhead -- --locked
scripts/bench_isolated.sh e4_memory_reclamation memory_policy_benchmarks oscillation_reclamation -- --locked
scripts/bench_isolated.sh e5_positioned_reads positioned_reads Positioned -- --features crc32 --locked
scripts/bench_isolated.sh e5_forward_position positioned_reads Tracking -- --features crc32 --locked

# Required rechecks recorded by the findings docs:
scripts/bench_isolated.sh a1_64b_recheck       write_pipeline_decomposition 64B -- --features crc32 --locked
scripts/bench_isolated.sh e1_file_recheck      vectored_framing 'file/default' -- --features crc32 --locked
scripts/bench_isolated.sh e1_bufwriter_recheck vectored_framing bufwriter -- --features crc32 --locked
```

Running the whole suite in one pass is not equivalent. On the development
laptop, unchanged code drifted −24% and +57% between consecutive full runs, and
that drift manufactured a 34% "win" that did not survive isolation
(`FINDINGS_VECTORED_FRAMING.md` threat T1). One group at a time, machine
otherwise idle.

Overwriting a file when re-collecting on different hardware is expected — the
header records which machine produced it, so `git log` carries the history.

## Current snapshots

| Snapshot | Cited by | State |
|---|---|---|
| `a1_write_pipeline.txt` | `FINDINGS_WRITE_PIPELINE_DECOMPOSITION.md` | complete |
| `a1_64b_recheck.txt` | `FINDINGS_WRITE_PIPELINE_DECOMPOSITION.md` threat T1 | complete; required CRC cross-check recheck |
| `e1_file.txt` | `FINDINGS_VECTORED_FRAMING.md` | complete |
| `e1_file_recheck.txt` | `FINDINGS_VECTORED_FRAMING.md` threat T1 | complete; required surprising-result recheck |
| `e1_tcp.txt` | `FINDINGS_VECTORED_FRAMING.md` | complete |
| `e1_bufwriter.txt` | `FINDINGS_VECTORED_FRAMING.md` | complete |
| `e1_bufwriter_recheck.txt` | `FINDINGS_VECTORED_FRAMING.md` threat T1 | complete; required surprising-result recheck |
| `e3_sync_dispatch.txt` | `FINDINGS_SYNC_POLICY.md` | complete |
| `e3_sync_file.txt` | `FINDINGS_SYNC_POLICY.md` | complete |
| `a1_durability.txt` | `FINDINGS_SYNC_POLICY.md` | complete |
| `e3_instruction_counts.txt` | historical E3 pre-memory-refactor snapshot | superseded by `e4_instruction_counts.txt` |
| `e4_memory_writer.txt` | `FINDINGS_STATIC_MEMORY_POLICY.md` | complete |
| `e4_memory_reader_recheck.txt` | `FINDINGS_STATIC_MEMORY_POLICY.md` | complete; required reader recheck |
| `e4_memory_reclamation.txt` | `FINDINGS_STATIC_MEMORY_POLICY.md` | complete |
| `e4_instruction_counts.txt` | `FINDINGS_STATIC_MEMORY_POLICY.md`, `FINDINGS_SYNC_POLICY.md` | complete; pinned Linux/Gungraun |
| `e5_positioned_reads.txt` | `FINDINGS_POSITIONED_READS.md` | complete |
| `e5_forward_position.txt` | `FINDINGS_POSITIONED_READS.md` | complete |
| `a2_position_accounting.txt` | `FINDINGS_POSITION_ACCOUNTING.md` | complete; pinned Linux/Gungraun |

Criterion snapshots above were collected one group at a time on the same Apple
M4 / macOS / Rust 1.97.1 environment stamped in each file. Instruction counts
use the pinned aarch64 Linux container recorded in their header. Absolute values
remain environment-specific; the findings documents state where repeated runs
were inconsistent rather than selecting the favourable result.
