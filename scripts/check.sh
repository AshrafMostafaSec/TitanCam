#!/usr/bin/env bash
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/env.sh"
cd "$TITANCAM_ROOT"
if [[ "$(rustc --version)" != "rustc 1.98.1 "* ]]; then
  echo 'Pinned Rust version mismatch. Rerun scripts/install-rust.sh.' >&2
  exit 1
fi
python3 -m py_compile scripts/desktop-launcher.py
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --release --locked
