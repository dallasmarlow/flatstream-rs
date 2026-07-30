#!/usr/bin/env bash
# Miri (nightly) over the library's in-src unit tests and the targeted
# positioned-read integration suite: undefined-behavior detection at the
# zero-copy buffer boundaries — the reader/writer pointer, borrowing, and
# length/offset arithmetic that ordinary tests execute but cannot prove sound.
#
# Scope remains deliberately targeted: `--lib` plus
# `--test positioned_reads`. The retained `BufReader<File>` case in that target
# is ignored under Miri because isolation forbids tempfile-backed filesystem
# access; gate.sh executes it natively. All in-memory caller-scratch borrowing,
# receipt-bound, one-byte-read, and partial-frame retry cases run under Miri.
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
    cargo +nightly miri test --locked --lib --test positioned_reads --features all_checksums
else
    echo "no rustup nightly on this machine — running inside the nightly Linux container"
    docker run --rm \
        -v "$PWD":/work -v flatstream-miri-target:/work/target \
        -w /work rustlang/rust:nightly bash -c "
        set -euo pipefail
        rustup component add miri >/dev/null
        cargo miri test --locked --lib --test positioned_reads --features all_checksums
    "
fi
