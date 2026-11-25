#!/bin/sh
set -e
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
cd "$(git rev-parse --show-toplevel)"
echo "checks: cargo fmt"
cargo fmt --all --check
echo "checks: cargo clippy"
cargo clippy --profile quick --workspace --all-targets --quiet -- -D warnings
echo "checks: cargo test"
cargo test --workspace --quiet
echo "checks: all passed"
