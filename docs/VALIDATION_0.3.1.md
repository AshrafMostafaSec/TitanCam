# TitanCam 0.3.1 / iPhone build 8 qualification

Date: 2026-10-01. Development release; update both endpoints. The iOS artifact is unsigned and requires local signing/provisioning in the user’s sideloading tool.

## Changes and rationale

The 0.3.0 camera/input controls, correlated configuration acknowledgement, independent A/V workers, affine clock mapping, audio resampling/gain/mute, bounded reordering/fragment repair and one-owner output model are retained.

0.3.1 defaults the desktop to **HEVC Main SDR, Balanced 1920×1080/60 FPS, 8 Mbit/s video**. Maximum HEVC starts at 24 Mbit/s; Saver HEVC at 3 Mbit/s. H.264 remains selectable. A 35 Mbit/s aggregate Wi-Fi wire budget caps peak pacing including packet headers, audio and repair; video bitrate is limited with 10% plus 0.4 Mbit/s allowance. It is a conservative starting setting for the reported 2.4 GHz network, not a measured capacity or quality guarantee. USB retains its separate settings. Actual camera formats override unsupported requests.

GPU/plugin/preview construction occurs before MediaReady so it does not run for the first time on live ingress. Decoder surface allocation still depends on the first encoded parameter sets; 4K startup remains a qualification limitation.

Skipping an unencoded camera frame under encoder pressure no longer forces an unnecessary IDR. Encoded reference loss still requires recovery. External IDR requests are limited to one per 250 ms. SenderStats reports raw capture drops and encoder skips separately from receiver counters.

Both endpoints report their version/build in the handshake. Capabilities have explicit ready/pending/unavailable/update-required states, with an explicit capability query when supported. The GUI displays the installed phone version and a useful update message instead of indefinitely waiting for resolutions. Camera/resolution controls work even if iOS exposes no optional mic data-source catalog. Front camera and actual lens/microphone choices remain catalog-driven, not model-name tables.

## Verification completed locally

- `scripts/check.sh`: Rust formatting, warnings-as-errors lint, **17 tests**, release compilation, Python compilation and desktop-file validation passed.
- `scripts/verify-protocol-fixtures.sh`: shared 64-byte wire fixture and JSON schemas passed; old configurations omit the optional wire budget and default to 35.
- Isolated GTK fixture check: actual mic vs system default; camera apply without an optional microphone catalog; update-required and disconnected controls disabled; stop returns in less than 1 ms while the event loop remains responsive.
- Generated HEVC hardware probes below exercise preview + `/dev/video42` webcam + native PipeWire mic + private-API gain/mute and image transforms simultaneously. All tone metering/control assertions passed.

Host: Ubuntu 26.04.1, GStreamer 1.28.2, PipeWire 1.6.2, RTX 4050 Laptop, NVIDIA driver 595.91.07. These are approximately five-second generated **localhost** fixtures, not iPhone, AP, image fidelity, sensor latency or endurance tests. Encoding the easy generated ball pattern is not a high-motion quality/bitrate test.

| Test | Generated / accepted / decoded frames | Aggregate drops, baseline → final | Decode FPS sample | Repair requests | Expired / missing units | Mic underruns, baseline → final |
|---|---:|---:|---:|---:|---:|---:|
| HEVC 1080p60 healthy | 300 / 300 / 300 | 0 → 0 | 60.01 | 0 | 0 / 0 | 18 → 18 |
| HEVC 1080p60 omitted fragment, repaired | 300 / 300 / 300 | 0 → 0 | 54.55 | 1 | 0 / 0 | 13 → 18 |
| HEVC 4K60 healthy | 300 / 286 / 282 | 14 → 14 | 59.99 | 0 | 0 / 0 | 13 → 15 |

Every test used `nvh265dec` and processed 499/499 synthetic audio packets. No late video/audio units were reported. The 4K startup discarded 13 dependent pictures following backpressure and requested two IDRs; startup drops are not hidden. The repair FPS sample is below 60 despite all frames being decoded, illustrating that the ending/window sample is not an uninterrupted output-FPS measurement. Counters count decoder buffers, not display/OBS consumption. PipeWire startup/end underruns remain counted; sustained audio stability is unqualified.

Reproduce with `source scripts/env.sh` then `XDG_RUNTIME_DIR=/run/user/1000 target/release/titan-bench --hardware --healthy --codec hevc --fps 60 --width 1920 --height 1080 --preview --webcam /dev/video42 --audio --controls`. Substitute `--repair` for `--healthy` or dimensions 3840×2160. Do not run against an active desktop receiver/output owner. Generated JSON reports accompany release assets.

## Cloud build gate

The iOS workflow uses macOS 15/Xcode 16.4, pinned Opus libraries, unsigned device compilation and the simulator suite (25 tests expected in this source). Linux workflows build/test/package separately for Ubuntu 24.04 and 26.04, with software H.264 recovery/healthy/repair and HEVC healthy fixtures. Publish artifacts only after both workflows succeed for the release commit; record exact run URLs and hashes in the final handoff. GitHub-hosted jobs do not validate the physical camera, USB, Wi-Fi, host NVIDIA driver or kernel module.

## Installation and physical acceptance

Install the matching Ubuntu DEB and locally re-sign/install the build-8 IPA. On connecting, verify the desktop reports `iPhone 0.3.1/8`, the actual camera/formats/mic catalog, HEVC and `nvh265dec`. Begin at Balanced; use Maximum only after measuring stable LAN throughput and thermal behavior. Internet download speed and the 2.4 GHz frequency are not LAN capacity measurements.

Remaining physical gates: phone front/rear/format/mic switching; Wi-Fi/AP and USB disconnect/recovery; steady high-motion 1080p60 and 4K60; OBS/browser devices; real flash/click A/V skew and glass-to-glass latency; color/fidelity; thermal/power endurance. Previous physical snapshot used Saver H.264 near 30 FPS and does not qualify this build. Do not promise zero latency or zero drops under arbitrary interference. Existing playout settings remain 35 ms USB and 70/60/65 ms Wi-Fi Saver/Balanced/Maximum, not measured end-to-end latency.

The trusted-LAN plaintext product contract remains unchanged. Foreground operation is required; HDR, capacity probing, FEC and recording remain deferred. NVIDIA performs hardware video decoding; network parsing, audio processing and V4L2 compatibility still use CPU work.
