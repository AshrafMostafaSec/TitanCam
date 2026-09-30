#!/usr/bin/env bash
# Source this file; installs stay inside this project and do not modify ~/.bashrc.
TITANCAM_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
export CARGO_HOME="$TITANCAM_ROOT/.dev/cargo"
export RUSTUP_HOME="$TITANCAM_ROOT/.dev/rustup"
export PATH="$CARGO_HOME/bin:$PATH"
