#!/usr/bin/env bash
# Miri (nightly) over the library's in-src unit tests: undefined-behavior
# detection at the zero-copy buffer boundaries — the reader/writer pointer and
# length arithmetic that ordinary tests execute but cannot prove sound.
#
# Scope: `--lib` only, deliberately. The integration suite runs natively in
# gate.sh; Miri's marginal value is concentrated in the unit tests that
# exercise the hot-path buffer handling directly, and `--lib` keeps the run
# fast enough to actually be run. Widen per-file if a boundary moves into an
# integration test (the slice-reader work is the expected trigger).
#
# Manually invoked locally; there is no CI schedule. Miri requires a nightly
# toolchain. When rustup with a nightly is present, it is used directly.
# Otherwise — e.g. this Homebrew-rust, no-rustup workstation — the run falls
# back to the official nightly Linux container (same pattern as fuzz.sh and
# instruction_counts.sh). Build artifacts go to a named volume so repeat runs
# are incremental.
set -euo pipefail
cd "$(dirname "$0")/.."

if command -v rustup >/dev/null 2>&1 && rustup toolchain list 2>/dev/null | grep -q nightly; then
    rustup component add miri --toolchain nightly >/dev/null
    cargo +nightly miri test --locked --lib --features all_checksums
else
    echo "no rustup nightly on this machine — running inside the nightly Linux container"
    docker run --rm \
        -v "$PWD":/work -v flatstream-miri-target:/work/target \
        -w /work rustlang/rust:nightly bash -c "
        set -euo pipefail
        rustup component add miri >/dev/null
        cargo miri test --locked --lib --features all_checksums
    "
fi
