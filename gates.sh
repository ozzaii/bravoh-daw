#!/usr/bin/env bash
# Full quality gates — the same checks CI runs. Green here = shippable.
set -euo pipefail
cd "$(dirname "$0")"

cargo test --all-targets
cargo clippy --all-targets -- -D warnings
cargo fmt --check

# Windows cross-compile check when the target is installed
# (rustup target add x86_64-pc-windows-msvc)
if rustup target list --installed | grep -q x86_64-pc-windows-msvc; then
    cargo check --all-targets --target x86_64-pc-windows-msvc
    cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings
fi

echo "ALL GATES GREEN"
