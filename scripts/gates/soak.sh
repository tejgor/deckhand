#!/bin/sh
# G8 soak gate: builds the release binary and runs scripts/gates/soak.mjs (60 minutes by default), which writes
# docs/port/gates/G8.md and keeps the raw samples, logs and UI screens in $SOAK_OUT (default: a new $TMPDIR directory).
# Everything runs in a temp sandbox; see the header of soak.mjs for the knobs (SOAK_MINUTES, SOAK_CYCLE, SOAK_SAMPLE, ...).
set -eu
project=$(CDPATH='' cd -- "$(dirname -- "$0")/../.." && pwd -P)
if [ -z "${SOAK_BIN:-}" ]; then
	cargo build --release -p deckhand --manifest-path "$project/Cargo.toml"
fi
exec node "$project/scripts/gates/soak.mjs" "$@"
