#!/usr/bin/env bash
# Pinned-environment instruction counts for four end-to-end workloads
# (write/read × default/xxh64) via iai-callgrind.
#
# Why: Criterion measures wall-clock, which on a workstation swings several
# percent run-to-run (thermal state, background load) — we have measured ±16%
# on layout-sensitive loops. Callgrind counts instructions, which are immune
# to that noise. Scope the claim correctly: counts are stable only for a
# pinned toolchain/dependencies/target/flags — that is why the container tag
# below is pinned, and why deltas are only meaningful against a previous run
# in the same environment (iai-callgrind stores them under target/iai/).
#
# Needs valgrind, i.e. Linux. On macOS this script runs the bench inside a
# Linux container (first run downloads/compiles; later runs reuse the local
# target-iai cache volume).
set -euo pipefail
cd "$(dirname "$0")/.."

# The runner version must match the iai-callgrind library in Cargo.lock.
IAI_VERSION=$(grep -A1 '^name = "iai-callgrind"$' Cargo.lock | grep version | cut -d'"' -f2)

if command -v valgrind >/dev/null 2>&1; then
    command -v iai-callgrind-runner >/dev/null 2>&1 ||
        cargo install iai-callgrind-runner --version "$IAI_VERSION"
    echo "== environment fingerprint (record with the baseline)"
    rustc -Vv
    valgrind --version
    iai-callgrind-runner --version
    cargo bench --bench instruction_count --features instruction_bench,all_checksums
else
    echo "no valgrind on this machine — running inside a versioned Linux container"
    # The compiler image tag is fixed, but tags/package repositories are not
    # immutable. Record the fingerprint printed below with every baseline.
    # Bump deliberately and expect all baselines to shift when you do.
    docker run --rm -v "$PWD":/work -v flatstream-iai-target:/work/target -w /work rust:1.87-bookworm bash -c "
        set -euo pipefail
        apt-get update -qq && apt-get install -y -qq valgrind >/dev/null
        cargo install -q iai-callgrind-runner --version $IAI_VERSION
        echo '== environment fingerprint (record with the baseline)'
        rustc -Vv
        valgrind --version
        iai-callgrind-runner --version
        cargo bench --bench instruction_count --features instruction_bench,all_checksums
    "
fi
