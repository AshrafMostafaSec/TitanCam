# TitanCam

A new iPhone 13 → Linux camera/audio system: hardware camera encoding, direct
USB and local Wi-Fi, NVIDIA/CPU video decoding, virtual webcam and PipeWire microphone.

**Development preview. Camera-to-computer operation and latency have not yet been
qualified on a physical iPhone.** CI compilation is a separate milestone from hardware
validation. Maximum 4K60 is experimental. See [AGENTS.md](AGENTS.md),
[ARCHITECTURE.md](ARCHITECTURE.md) and [implementation status](docs/NEXT_STEPS.md).

## Linux

Separate x86_64 packages are built and tested on Ubuntu 24.04 and 26.04 in GitHub
Actions. Download the package matching your Ubuntu version (`lsb_release -rs`).
Their native USB library ABIs differ; do not install a 24.04 binary on 26.04.

```sh
./scripts/install-system-deps.sh
./scripts/install-rust.sh
source scripts/env.sh
./scripts/check.sh
cargo run --locked -p titan-receiver -- doctor --decode --microphone
```

Dependency setup uses sudo in your terminal. The receiver runs as your normal user.
For a release package use `sudo apt install ./titancam_0.2.0_ubuntu26.04_amd64.deb`
(on Ubuntu 24.04 select the corresponding `ubuntu24.04` package). NVIDIA drivers,
usbmuxd/device trust and v4l2loopback are separate host dependencies.

After installing the Debian package, open **TitanCam** from the application menu.
The desktop controller starts immediately and remains available when its window closes.
It offers profile/output controls and actual receiver counters. The iPhone is a simple
sender with USB, discovered computer buttons and Stop; there is no phone preview,
login, pairing code or certificate setup. Local transport is intentionally unencrypted.
Both ends must run **0.2.0 or newer**; the earlier encrypted app is incompatible.

### Connect

1. Open TitanCam on Linux.
2. Open the new TitanCam on the iPhone and allow camera/microphone/local network.
3. For USB, connect/trust the cable and tap **Connect with USB**. For Wi-Fi, use the
   same LAN and tap the computer name that appears.
4. Keep the phone app open. Choose Saver, Balanced or Maximum on Linux.

For CLI use:

```sh
titan-receiver local --advertise YOUR_LAN_IP --profile saver --preview
```

This automatically monitors USB and publishes Bonjour. Without a LAN address, omit
`--advertise` for USB. Allow TCP 49160, UDP 49161 and mDNS UDP 5353 on the trusted LAN
interface/subnet. Do not disable your whole firewall. USB listener conflicts fall
back between bases 43052/43062/43072 automatically. No copied credentials are needed.

### Application outputs

Install the optional module using `scripts/setup-virtual-camera.sh`, then load a
chosen device, for example `sudo modprobe v4l2loopback video_nr=42 exclusive_caps=1
card_label=TitanCam`. Add `--webcam /dev/video42` to the receiver. Choose **TitanCam
Microphone** in OBS/browser audio settings. Use `--mute` or `--software` for diagnostics.
`stats` shows receiver counters; `devices` lists attached phones privately.

## iPhone build and local installation

The iOS workflow uses **GitHub-hosted macOS 15 + Xcode 16.4**. This Linux PC does
not need macOS or Xcode. The workflow compiles a physical-device app and runs
simulator protocol/security tests. Hardware camera, USB and QUIC interoperability
still need a real phone test.

The default artifact is `TitanCam-unsigned-for-local-resigning.ipa`. It is **not
installable until re-signed and provisioned**. Import it into the user's local
[iLoader](https://github.com/nab138/iloader) and follow that tool's signing flow;
Apple credentials remain local. See [build/signing](docs/BUILD_AND_SIGNING.md).

## Website and builds

[GitHub repository](https://github.com/AshrafMostafaSec/TitanCam) ·
[Build runs](https://github.com/AshrafMostafaSec/TitanCam/actions) ·
[Application website](https://AshrafMostafaSec.github.io/TitanCam/) ·
[Releases](https://github.com/AshrafMostafaSec/TitanCam/releases)

No claim of zero latency or better-than-sensor quality. Profile values are targets;
measured p50/p95/p99 and A/V skew will be published after the hardware acceptance run.
