#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/env.sh
cargo build --release --locked -p titan-receiver
version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)"
source /etc/os-release
if [[ "$ID" != ubuntu || "$VERSION_ID" != 24.04 && "$VERSION_ID" != 26.04 ]]; then
  echo "Packaging targets only Ubuntu 24.04 and 26.04" >&2; exit 1
fi
platform="ubuntu${VERSION_ID}"
architecture="$(dpkg --print-architecture)"
[[ "$architecture" == amd64 ]] || { echo "Only amd64 packages are supported" >&2; exit 1; }
staging="build/package-root"
rm -rf "$staging" build/package
mkdir -p "$staging/usr/bin" "$staging/usr/share/doc/titancam" "$staging/DEBIAN" build/package
install -m 755 target/release/titan-receiver "$staging/usr/bin/"
mkdir -p "$staging/usr/share/titancam" "$staging/usr/share/applications"
install -m 644 scripts/desktop-launcher.py "$staging/usr/share/titancam/"
install -m 644 packaging/titancam.desktop "$staging/usr/share/applications/"
printf '%s\n' '#!/bin/sh' 'exec /usr/bin/python3 /usr/share/titancam/desktop-launcher.py "$@"' > "$staging/usr/bin/titancam"
chmod 755 "$staging/usr/bin/titancam"
cp README.md ARCHITECTURE.md AGENTS.md "$staging/usr/share/doc/titancam/"
cargo metadata --locked --format-version 1 > build/package/rust-dependency-manifest.json
python3 scripts/dependency-inventory.py build/package/rust-dependency-manifest.json build/package/TitanCam-Rust.cdx.json build/package/THIRD-PARTY-NOTICES.txt
cat LICENSE build/package/THIRD-PARTY-NOTICES.txt > "$staging/usr/share/doc/titancam/copyright"
cp build/package/TitanCam-Rust.cdx.json "$staging/usr/share/doc/titancam/"
mkdir -p build/dependency-scan/debian
cat > build/dependency-scan/debian/control <<CONTROL
Source: titancam
Section: video
Priority: optional
Maintainer: Ashraf Mostafa <233173916+AshrafMostafaSec@users.noreply.github.com>

Package: titancam
Architecture: any
Description: TitanCam receiver
CONTROL
native_dependencies="$(cd build/dependency-scan && dpkg-shlibdeps -O -e "$(pwd)/../package-root/usr/bin/titan-receiver" | sed -n 's/^shlibs:Depends=//p')"
[[ -n "$native_dependencies" ]] || { echo "Native dependency scan failed" >&2; exit 1; }
cat > "$staging/DEBIAN/control" <<CONTROL
Package: titancam
Version: $version
Section: video
Priority: optional
Architecture: $architecture
Maintainer: Ashraf Mostafa <233173916+AshrafMostafaSec@users.noreply.github.com>
Depends: $native_dependencies, python3-gi, gir1.2-gtk-4.0, avahi-daemon, avahi-utils, gstreamer1.0-plugins-base, gstreamer1.0-plugins-good, gstreamer1.0-plugins-bad, gstreamer1.0-libav
Description: TitanCam local iPhone camera receiver (development preview)
 Hardware streaming qualification is pending. NVIDIA drivers and the optional
 v4l2loopback kernel module are installed separately.
CONTROL
dpkg-deb --root-owner-group --build "$staging" "build/package/titancam_${version}_${platform}_amd64.deb"
tar -czf "build/package/titancam-linux-${platform}-x86_64.tar.gz" -C target/release titan-receiver -C "$(pwd)" README.md ARCHITECTURE.md LICENSE -C "$(pwd)/build/package" THIRD-PARTY-NOTICES.txt
(cd build/package && sha256sum *.deb *.tar.gz *.json THIRD-PARTY-NOTICES.txt > SHA256SUMS.txt)
