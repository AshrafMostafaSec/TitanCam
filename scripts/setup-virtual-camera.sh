#!/usr/bin/env bash
set -euo pipefail
# Separate optional step; does not alter the NVIDIA driver or disable Secure Boot.
sudo apt-get install --yes --no-install-recommends v4l2loopback-dkms "linux-headers-$(uname -r)"
echo 'Installed only. Module loading/caps/device selection wait for the receiver implementation.'
echo 'If Secure Boot requires module signing/MOK enrollment, follow the package instructions.'
