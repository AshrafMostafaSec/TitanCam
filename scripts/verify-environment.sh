#!/usr/bin/env bash
set -uo pipefail
source "$(dirname -- "${BASH_SOURCE[0]}")/env.sh"
missing=0
for tool in cargo rustc gcc cmake pkg-config git; do
  if command -v "$tool" >/dev/null && "$tool" --version >/dev/null 2>&1; then echo "OK tool: $tool"; else echo "MISSING tool: $tool"; missing=$((missing+1)); fi
done
for library in gstreamer-1.0 gstreamer-app-1.0 gstreamer-video-1.0 libpipewire-0.3 libusbmuxd-2.0 opus; do
  if version=$(pkg-config --modversion "$library" 2>/dev/null); then echo "OK library: $library $version"; else echo "MISSING library: $library"; missing=$((missing+1)); fi
done
for element in nvh264dec nvh265dec; do
  if gst-inspect-1.0 "$element" >/dev/null 2>&1; then echo "AVAILABLE factory: $element (actual decode still unverified)"; else echo "OPTIONAL unavailable: $element"; fi
done
systemctl --user is-active pipewire wireplumber || true
systemctl is-active usbmuxd || true
echo "Missing required tools/libraries: $missing"
exit "$((missing > 0))"
