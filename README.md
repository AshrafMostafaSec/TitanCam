# TitanCam

A new iPhone 13 → Linux camera/audio system: hardware camera encoding, authenticated
USB and local Wi-Fi, NVIDIA/CPU video decoding, virtual webcam and PipeWire microphone.

**Development preview. Camera-to-computer operation and latency have not yet been
qualified on a physical iPhone.** CI compilation is a separate milestone from hardware
validation. Maximum 4K60 is experimental. See [AGENTS.md](AGENTS.md),
[ARCHITECTURE.md](ARCHITECTURE.md) and [implementation status](docs/NEXT_STEPS.md).

## Linux

Ubuntu 24.04 LTS x86_64 is the CI/package baseline. Newer Ubuntu needs runtime testing.

```sh
./scripts/install-system-deps.sh
./scripts/install-rust.sh
source scripts/env.sh
./scripts/check.sh
cargo run --locked -p titan-receiver -- doctor --decode --microphone
```

Dependency setup uses sudo in your terminal. The receiver runs as your normal user.
For a release package use `sudo apt install ./titancam_0.1.0_amd64.deb`. NVIDIA drivers,
usbmuxd/device trust and v4l2loopback are separate host dependencies.

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
titan-receiver usb --phone-pin PHONE_CERTIFICATE_SHA256 --pair-token PHONE_ONE_TIME_TOKEN --profile saver --preview
```

A single attached phone is selected automatically. Multiple phones require explicit
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
