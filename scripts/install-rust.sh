#!/usr/bin/env bash
set -euo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/env.sh"
cd "$TITANCAM_ROOT"
# Verified, project-local installation; no shell startup files are modified.
case "$(uname -s):$(uname -m)" in
  Linux:x86_64) host=x86_64-unknown-linux-gnu ;;
  *) echo 'This bootstrap currently supports Linux x86_64 only.' >&2; exit 1 ;;
esac
mkdir -p .dev/downloads
if ! test -x "$CARGO_HOME/bin/rustup"; then
  url="https://static.rust-lang.org/rustup/dist/$host/rustup-init"
  curl --proto '=https' --tlsv1.2 --connect-timeout 20 --max-time 300 -fsSL "$url" -o .dev/downloads/rustup-init
  curl --proto '=https' --tlsv1.2 --connect-timeout 20 --max-time 60 -fsSL "$url.sha256" -o .dev/downloads/rustup-init.sha256
  (cd .dev/downloads && sha256sum -c rustup-init.sha256)
  chmod +x .dev/downloads/rustup-init
  .dev/downloads/rustup-init -y --no-modify-path --profile minimal --default-toolchain none
fi
# Reuse the exact stable installation if it already contains this pinned release.
# A named custom alias avoids downloading the identical compiler twice.
if [[ "$(rustup run stable rustc --version 2>/dev/null)" == "rustc 1.98.1 "* ]]; then
  source_toolchain=stable
  rustup component add --toolchain stable rustfmt clippy
else
  source_toolchain=1.98.1
  rustup toolchain install "$source_toolchain" --profile minimal --component rustfmt --component clippy
fi
compiler_root="$(rustup run "$source_toolchain" rustc --print sysroot)"
rustup toolchain link titancam-1.98.1 "$compiler_root"
# Do not update the backing toolchain independently: rerun this script to validate it.
rustc --version
cargo --version
