# Implementation status and hardware acceptance

Implemented source: AVFoundation camera/mic capture; required VideoToolbox hardware
H.264/HEVC; bounded encoder queues; Annex-B access units; PCM/Opus audio; native
Network.framework TLS and QUIC datagrams; pinned TLS + Ed25519 mutual session identity;
USB listeners/libusbmuxd connections; deadline-limited reassembly; clock exchange;
NVDEC with CPU fallback; GStreamer preview/V4L2; native real-time PipeWire source;
receiver telemetry; reconnect/backoff; thermal/network bitrate reduction; profile selection;
SwiftUI foreground UI; unsigned iOS GitHub build; Linux package and GitHub Pages workflows.

Locally passed: Rust format/lint and eight protocol/security/drift tests; RTX 4050 actual
H.264 hardware decode probe with 30 generated frames. A PipeWire source was created;
a generated tone reached a real PipeWire consumer. Physical-camera A/V sync has not yet been qualified. CI results appear
in GitHub Actions. Do not infer physical-phone success from source or compilation.

Required next hardware steps, after local signing/install:
1. Probe effective camera format and hardware encoder on iPhone 13.
2. USB saver 720p30 session for ten minutes, cable disconnect/reconnect and slow output.
3. Confirm webcam + mic in OBS and browser; measure flash/click A/V skew.
4. Validate native Network.framework QUIC DATAGRAM ↔ Quinn on actual iOS/network.
5. Wi-Fi balanced session and documented loss/jitter/outage matrix.
6. Each profile for 60 minutes, thermal/power/quality and external glass-to-glass latency.

Remaining enhancements: mDNS/QR scanning, front-camera switching, larger color/HDR
qualification, deadline-aware repair/FEC, recording, full calibrated drift model,
clock uncertainty display, broad fuzzing, combined binary/system/iOS SBOM review and clean-host installation
qualification. These are not presented as verified features. Maximum 4K60/HEVC remains
experimental until the corresponding hardware measurements pass.
