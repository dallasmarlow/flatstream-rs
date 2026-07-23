#!/usr/bin/env bash
# Pinned-environment instruction counts for four end-to-end workloads
# (write/read × default/xxh64) via Gungraun.
#
# Why: Criterion measures wall-clock, which on a workstation swings several
# percent run-to-run (thermal state, background load) — we have measured ±16%
# on layout-sensitive loops. Callgrind counts instructions, which are immune
# to that noise. Scope the claim correctly: counts are stable only for a
# pinned toolchain/dependencies/target/flags — that is why the container tag
# below is pinned, and why deltas are only meaningful against a previous run
# in the same environment (Gungraun stores them under target/gungraun/).
#
# Needs valgrind, i.e. Linux. On macOS this script runs the bench inside a
# Linux container (first run downloads/compiles; later runs reuse the local
# target cache volume).
set -euo pipefail
cd "$(dirname "$0")/.."

# The runner version must match the Gungraun library in Cargo.lock.
GUNGRAUN_VERSION=$(grep -A1 '^name = "gungraun"$' Cargo.lock | grep version | cut -d'"' -f2)
if [[ -z "$GUNGRAUN_VERSION" ]]; then
    echo "could not determine the Gungraun version from Cargo.lock" >&2
    exit 1
fi

if command -v valgrind >/dev/null 2>&1; then
    INSTALLED_VERSION=$(
        gungraun-runner --version 2>/dev/null | awk '{print $2}' || true
    )
    if [[ "$INSTALLED_VERSION" != "$GUNGRAUN_VERSION" ]]; then
        cargo install --locked --force gungraun-runner --version "$GUNGRAUN_VERSION"
    fi
    echo "== environment fingerprint (record with the baseline)"
    rustc -Vv
    valgrind --version
    gungraun-runner --version
    cargo bench --locked --bench instruction_count --features instruction_bench,all_checksums
else
    echo "no valgrind on this machine — running inside a versioned Linux container"
    # The compiler image tag is fixed, but tags/package repositories are not
    # immutable. Record the fingerprint printed below with every baseline.
    # Bump deliberately and expect all baselines to shift when you do.
    # Gungraun uses `setarch -R` to disable ASLR; Docker's default seccomp
    # profile blocks the required personality syscall. This trusted,
    # short-lived benchmark container therefore runs with seccomp unconfined.
    docker run --rm --security-opt seccomp=unconfined \
        -v "$PWD":/work -v flatstream-gungraun-target:/work/target \
        -w /work rust:1.87-bookworm bash -c "
        set -euo pipefail
        apt-get update -qq && apt-get install -y -qq valgrind >/dev/null
        cargo install -q --locked gungraun-runner --version $GUNGRAUN_VERSION
        echo '== environment fingerprint (record with the baseline)'
        rustc -Vv
        valgrind --version
        gungraun-runner --version
        cargo bench --locked --bench instruction_count --features instruction_bench,all_checksums
    "
fi
