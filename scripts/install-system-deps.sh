#!/usr/bin/env bash
set -euo pipefail
# Run from a local terminal: sudo authenticates there; never provide passwords to chat.
packages=(build-essential dpkg-dev cmake ninja-build pkg-config curl ca-certificates git
  desktop-file-utils clang libclang-dev libssl-dev libudev-dev libgstreamer1.0-dev
  libgstreamer-plugins-base1.0-dev libpipewire-0.3-dev libusbmuxd-dev
  libopus-dev gstreamer1.0-plugins-base gstreamer1.0-plugins-good
  gstreamer1.0-plugins-bad gstreamer1.0-libav libusbmuxd-tools
  usbmuxd libimobiledevice-utils v4l-utils avahi-utils)
sudo apt-get update
if apt-cache show libimobiledevice-glue-dev >/dev/null 2>&1; then packages+=(libimobiledevice-glue-dev); fi
sudo apt-get install --yes --no-install-recommends "${packages[@]}"
echo 'Development dependencies installed. Run scripts/verify-environment.sh next.'
