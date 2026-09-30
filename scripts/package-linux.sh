#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/env.sh
cargo build --release --locked -p titan-receiver
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
staging="build/package-root"
rm -rf "$staging"
mkdir -p "$staging/usr/bin" "$staging/usr/share/doc/titancam" "$staging/DEBIAN" build/package
install -m 755 target/release/titan-receiver "$staging/usr/bin/"
cp README.md ARCHITECTURE.md AGENTS.md "$staging/usr/share/doc/titancam/"
cargo metadata --locked --format-version 1 > build/package/rust-dependency-manifest.json
python3 scripts/dependency-inventory.py build/package/rust-dependency-manifest.json build/package/TitanCam-Rust.cdx.json build/package/THIRD-PARTY-NOTICES.txt
cat LICENSE build/package/THIRD-PARTY-NOTICES.txt > "$staging/usr/share/doc/titancam/copyright"
cp build/package/TitanCam-Rust.cdx.json "$staging/usr/share/doc/titancam/"
cat > "$staging/DEBIAN/control" <<CONTROL
Package: titancam
Version: $version
Section: video
Priority: optional
Architecture: amd64
Maintainer: Ashraf Mostafa <233173916+AshrafMostafaSec@users.noreply.github.com>
Depends: libc6, libusbmuxd6, libpipewire-0.3-0, libopus0, libgstreamer1.0-0, libgstreamer-plugins-base1.0-0, gstreamer1.0-plugins-base, gstreamer1.0-plugins-good, gstreamer1.0-plugins-bad, gstreamer1.0-libav
Description: TitanCam encrypted iPhone camera receiver (development preview)
 Hardware streaming qualification is pending. NVIDIA drivers and the optional
 v4l2loopback kernel module are installed separately.
CONTROL
dpkg-deb --root-owner-group --build "$staging" "build/package/titancam_${version}_amd64.deb"
tar -czf "build/package/titancam-linux-x86_64.tar.gz" -C target/release titan-receiver -C "$(pwd)" README.md ARCHITECTURE.md LICENSE -C "$(pwd)/build/package" THIRD-PARTY-NOTICES.txt
(cd build/package && sha256sum *.deb *.tar.gz *.json THIRD-PARTY-NOTICES.txt > SHA256SUMS.txt)
