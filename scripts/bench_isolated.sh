#!/usr/bin/env bash
# Run one benchmark group at a time and commit the raw output.
#
# Why one group at a time: on the development laptop, unchanged code moved by
# -24% and +57% between consecutive full-suite runs, and the drift produced a
# false 34% "win" that nearly reached a findings doc. Criterion reports such
# drift as statistically significant because it compares against its own saved
# baseline and cannot tell that the machine, not the code, changed. See
# docs/benchmark/FINDINGS_VECTORED_FRAMING.md threat T1.
#
# The rule that follows: only A-vs-B pairs collected inside a single isolated
# run are admissible, and a surprising delta gets re-collected before it is
# written down. This script makes the isolated run the easy path and drops the
# raw output where a findings doc can cite it (the evidence standard requires a
# committed snapshot, not a hand-summary).
#
# Usage:
#   scripts/bench_isolated.sh <slug> <bench-name> [criterion-filter] [-- cargo-args...]
#
# Examples:
#   scripts/bench_isolated.sh e1_file       vectored_framing 'file/'
#   scripts/bench_isolated.sh e1_bufwriter  vectored_framing bufwriter
#   scripts/bench_isolated.sh a1_write_pipeline write_pipeline_decomposition '' -- --features crc32
set -euo pipefail
cd "$(dirname "$0")/.."

if [ $# -lt 2 ]; then
    sed -n '2,23p' "$0" >&2
    exit 2
fi

SLUG=$1
BENCH=$2
FILTER=${3:-}
shift $(( $# < 3 ? $# : 3 ))
if [ "${1:-}" = "--" ]; then shift; fi
CARGO_ARGS=("$@")
HAS_LOCKED=0
for arg in "${CARGO_ARGS[@]}"; do
    if [[ "$arg" == "--locked" ]]; then HAS_LOCKED=1; fi
done
if [[ "$HAS_LOCKED" == "0" ]]; then
    CARGO_ARGS=(--locked "${CARGO_ARGS[@]}")
fi

OUT_DIR=docs/benchmark/raw
mkdir -p "$OUT_DIR"
OUT="$OUT_DIR/$SLUG.txt"

CMD=(cargo bench "${CARGO_ARGS[@]}" --bench "$BENCH")
if [[ -n "$FILTER" ]]; then
    CMD+=(-- "$FILTER")
fi

CPU=unknown
if [[ "$(uname -s)" == "Darwin" ]]; then
    CPU=$(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo unknown)
elif [[ -r /proc/cpuinfo ]]; then
    CPU=$(awk -F: '/model name|Hardware/ { sub(/^[[:space:]]+/, "", $2); print $2; exit }' /proc/cpuinfo)
    CPU=${CPU:-unknown}
fi

{
    echo "# $SLUG"
    echo "# date:    $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
    echo "# rustc:   $(rustc --version)"
    echo "# host:    $(uname -srm)"
    echo "# cpu:     $CPU"
    echo "# git:     $(git rev-parse HEAD)"
    if [[ -n "$(git status --porcelain)" ]]; then
        echo "# dirty:   yes"
    else
        echo "# dirty:   no"
    fi
    echo "# lock:    $(python3 -c 'import hashlib; print(hashlib.sha256(open("Cargo.lock", "rb").read()).hexdigest())')"
    echo "# selector: A4_CASE=${A4_CASE:-} POSITIONED_READS_CASE=${POSITIONED_READS_CASE:-}"
    printf '# command:'
    printf ' %q' "${CMD[@]}"
    printf '\n'
    echo
} >"$OUT"

echo "== $SLUG -> $OUT"
echo "   Close other work first; this measurement is only as good as the machine is idle."
"${CMD[@]}" 2>&1 | tee -a "$OUT"

# Criterion separates benchmark groups with blank lines. Keep the raw snapshot
# newline-terminated without trailing blank records so `git diff --check` stays
# meaningful for generated evidence too.
python3 - "$OUT" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
path.write_text(path.read_text().rstrip() + "\n")
PY

echo "== wrote $OUT"
