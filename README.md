# TitanCam

A new iPhone 13 → Linux camera/audio system: hardware camera encoding, authenticated
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
For a release package use `sudo apt install ./titancam_0.1.0_ubuntu26.04_amd64.deb`
(on Ubuntu 24.04 select the corresponding `ubuntu24.04` package). NVIDIA drivers,
usbmuxd/device trust and v4l2loopback are separate host dependencies.

After installing the Debian package, open **TitanCam** from the application menu.
The local desktop controller offers USB/Wi-Fi, profile and output selection, a temporary
Wi-Fi QR code (scan with the standard iPhone Camera, then tap Connect in TitanCam),
and actual receiver counters. It keeps pairing credentials out of diagnostic logs;
QR files are private and expire after 120 seconds. CLI remains available below.

### Wi-Fi

```sh
titan-receiver run --bind 0.0.0.0 --advertise YOUR_LAN_IP --pairing --profile balanced --preview
```

Open TitanCam on the phone, allow camera/microphone/local network, and paste the
pairing link shown by the receiver. First pairing expires after 120 seconds. Keep
the app in the foreground. Permit TCP 49160 and UDP 49161 only on your trusted LAN.
Profiles: `saver`, `balanced`, `maximum`. Connect the PC to the AP by Ethernet when possible.

### USB

Trust the Linux computer on the phone. Tap **Enable USB connection** in TitanCam,
then use the certificate pin and temporary token displayed on the phone:

```sh
titan-receiver usb --usb-base-port 43052 --phone-pin PHONE_CERTIFICATE_SHA256 --pair-token PHONE_ONE_TIME_TOKEN --profile saver --preview
```

Use the USB base port displayed by the phone; it normally is `43052` and can fall
back to `43062` or `43072`. Alpha.1 phones use `--usb-base-port 49152`. Stop/restart
and USB/Wi-Fi switching await listener shutdown. A single attached phone is selected automatically. Multiple phones require explicit
`--udid`; never upload that value in an issue or ordinary diagnostics.

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
