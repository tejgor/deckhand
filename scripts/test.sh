#!/bin/sh
# Build the executable before the black-box tests; prefer nextest when installed.
set -eu

cd "$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
cargo build --workspace
if cargo nextest --version >/dev/null 2>&1; then
    cargo nextest run --workspace "$@"
else
    cargo test --workspace "$@"
fi
