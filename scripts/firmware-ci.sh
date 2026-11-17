#!/usr/bin/env bash
# Firmware checks (separate workspace, Xtensa `esp` toolchain). Usage: scripts/firmware-ci.sh
# Run by scripts/ci.sh and by CI's firmware job.
set -euo pipefail
cd "$(dirname "$0")/../firmware/esp32s3"
# The Xtensa linker comes from the espup environment.
[ -f "$HOME/export-esp.sh" ] && . "$HOME/export-esp.sh"
step() { printf '\n-- firmware: %s\n' "$*"; }

step "fmt"
cargo fmt -- --check

step "clippy"
cargo clippy --quiet --release -- -D warnings

# The diagnostics behind `debug-log` are compiled by no other step.
step "clippy with debug-log"
cargo clippy --quiet --release --features debug-log -- -D warnings

step "build"
cargo build --quiet --release

step "image size"
../../scripts/size.sh
