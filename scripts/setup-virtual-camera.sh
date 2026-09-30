#!/usr/bin/env bash
set -euo pipefail
# Separate optional step; does not alter the NVIDIA driver or disable Secure Boot.
sudo apt-get install --yes --no-install-recommends v4l2loopback-dkms "linux-headers-$(uname -r)"
echo 'Installed only. To create a device: sudo modprobe v4l2loopback video_nr=42 exclusive_caps=1 card_label=TitanCam'
echo 'Add --webcam /dev/video42 to the receiver; choose TitanCam Microphone in your audio settings.'
echo 'If Secure Boot requires module signing/MOK enrollment, follow the package instructions.'
