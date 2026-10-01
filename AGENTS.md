# TitanCam — coding-agent contract

Research baseline: **2026-09-30**; implementation decision updated **2026-10-01**.

**Existing-project improvement scope (2026-10-01):** 0.3.0 implements acknowledged Linux controls for discovered front/rear camera formats, microphone input/data-source selection, digital gain/mute and image transforms; independent A/V workers, drift mapping and bounded Wi-Fi repair/reordering. Follow [IMPROVEMENT_PLAN.md](docs/IMPROVEMENT_PLAN.md) and the current implementation update in ARCHITECTURE.md. Maximum defaults to H.264 65 Mbit/s; HEVC is selectable with lower starting rates. The owner authorized implementation/builds and deferred physical two-hour testing. Preserve the local-v2 plaintext product contract unless the owner changes it; QUIC/WebRTC are evaluated alternatives, not authorized transport migrations. Do not label CI or synthetic hardware probes as physical 4K60, latency, quality or endurance qualification.

**Authoritative local v2 scope:** The human owner explicitly requested removal of authentication, encryption, pairing codes, and the iPhone preview for their trusted company LAN. Implement clear TCP control/USB and paced UDP media with Bonjour discovery. Do not reintroduce TLS, QUIC, signatures, certificate/Keychain setup or pairing UI. Keep strict bounds, random session IDs and automatic channel binding for correctness; they provide no confidentiality or peer authentication. Both endpoints must run 0.2.0+; older encrypted applications are incompatible. iPhone is sender-only; Linux owns profile/output controls and actual health. This supersedes older research/security provisions below. Companion specification: [ARCHITECTURE.md](ARCHITECTURE.md). Copy both files to the repository root before implementation. This file defines engineering requirements; ARCHITECTURE.md defines the system, protocol, operating profiles, experiments, and source evidence.

## 1. Deliverable and priorities

Build a new foreground iPhone 13 camera/microphone application and an Ubuntu/Linux receiver. Deliver direct USB and local Wi-Fi streaming, a low-delay Linux preview, a virtual webcam, a virtual microphone, diagnostics, automated builds, and installable artifacts. Project name and bundle ID are configurable; TitanCam is a working name.

Order of priorities: correctness and security; bounded latency and recovery; faithful image/audio quality; sustained stability; measured energy efficiency; optional enhancements. Do not optimize a benchmark by hiding dropped frames, reducing effective resolution, measuring only network time.

**Never promise zero latency, lossless compressed video, delivery of every live frame on unreliable Wi-Fi, indefinite 4K60 operation, or image quality superior to the original sensor output.** Capture exposure, codec work, buffering, display scanout, and downstream applications all contribute delay. The operating-profile values are starting hypotheses and acceptance targets, not measured results.

Implement usable increments. Do not deliver a UI containing simulated telemetry as though it were live, placeholder transports, unbounded queues, or a receiver that only plays a sample file. Clearly label simulator fixtures and experimental features.

## 2. Required stack and compatibility

- iOS: Swift, SwiftUI, AVFoundation, CoreMedia/CoreVideo, VideoToolbox, Network.framework, Security for random session data; implemented deployment target iOS 17.4 or newer (required hardware-encoder APIs). Availability-check each API against the selected SDK and physical-device OS. Hardware validation uses iPhone 13, not a simulator.
- Linux: Rust workspace; Tokio for non-real-time orchestration, plain TCP/UDP for local networking, gstreamer-rs for video, PipeWire for audio-source publication, and a narrow libusbmuxd C FFI adapter for USB. A small C shim is acceptable if safer than handwritten ABI bindings.
- Baseline distribution: Ubuntu 24.04 LTS x86_64, GStreamer 1.24 or newer. Add newer Ubuntu releases only after packaging/runtime validation. Pin Rust and dependency versions compatible with that baseline; do not enable bindings for newer GStreamer properties without feature detection.
- Preferred GPU: actual RTX 4050 **Laptop** GPU, verified by PCI/device/driver diagnostics. NVDEC via `nvh264dec`/`nvh265dec`; software fallback is required. NVENC is unnecessary for the normal receive/decode path.
- Virtual webcam: v4l2loopback with DKMS/kernel-header installation handled separately from the unprivileged receiver. Virtual mic: a native PipeWire `Audio/Source` stream. PipeWire video publication is optional after the V4L2 compatibility path works.
- Initial codecs: hardware H.264, 8-bit 4:2:0 SDR; HEVC Main as a measured upgrade; Opus 48 kHz for Wi-Fi; PCM 48 kHz S16LE for USB. HDR/10-bit is an opt-in later capability, with a separate SDR conversion path.
- Preserve `Cargo.lock`, `Package.resolved`, project-generation tool versions if used, CI action commit pins, package manifests, codec library source checksums, and a tested compatibility matrix. Do not guess the newest Xcode, driver, package, or runner version.

The detailed source links and technology caveats are in ARCHITECTURE.md. Recheck upstream API documentation when changing these decisions. A research citation is evidence of an API or capability, not evidence that TitanCam already achieves a performance target.

## 3. Repository layout

```text
AGENTS.md
ARCHITECTURE.md
ios/TitanCam.xcodeproj/         # shared TitanCam scheme, committed project
ios/TitanCam/                  # app and reusable Swift modules
ios/TitanCamTests/
crates/titan-protocol/         # pure parsing, schemas, golden vectors
crates/titan-usb/              # libusbmuxd ownership and event adapter
crates/titan-transport/        # bounded local-v2 TCP record I/O and session helpers
crates/titan-media/            # GStreamer, clocks, PipeWire output
crates/titan-receiver/         # CLI, orchestration, local control socket
crates/titan-bench/            # synthetic sender, impairment/replay tools
protocol/                     # versioned JSON schemas + binary fixtures
config/profiles/               # saver/balanced/maximum TOML defaults
packaging/                    # Debian metadata, user service, module guidance
scripts/                      # build, verify, package, bench entry points
tests/fixtures/                # generated/licensed media only
docs/                         # operation, compatibility, benchmark reports, ADRs
.github/workflows/
```

Module contracts: capture emits timestamped frames; encoder emits codec configuration and complete access units; transport accepts bounded messages with deadlines; receiver transport emits validated complete units; media owns presentation clocks/output. UI never owns sockets or encoder sessions. Pure protocol code must compile/test without cameras, GStreamer, PipeWire, or NVIDIA drivers.

## 4. Mandatory implementation invariants

1. Use one serialized capture-session owner. No synchronous camera start/configuration on the main thread. Keep capture/audio callbacks short; explicit buffer ownership; no blocking socket sends in callbacks.
2. Every queue has byte/item limits, age limits, overload counters, and a written overflow policy. Do not drop arbitrary compressed P-frames and continue decoding dependent pictures. Flush damaged video, request an IDR, and resume at an independent frame.
3. All samples use one normalized sender capture timeline. Map it to receiver monotonic time with offset/drift estimation. Never infer A/V sync from arrival order or wall-clock timestamps.
4. All sessions have random IDs; transport reconnects and configuration changes have epochs. Reject stale data. Reset timestamps safely after camera/audio resets.
5. Parameters are negotiated and reported as **requested**, **effective**, and **reason for fallback**. Query supported VideoToolbox properties; log all failed setters. Hardware use must be confirmed: require hardware at creation, prepare resources, then inspect the optional hardware property status/type/value. If that property is unsupported or absent, the mandatory-hardware creation contract is the recorded evidence; never silently fall back to software. An explicit false value or unexpected query error fails.
6. Video has no B-frame reordering in low-delay profiles. H.264's dedicated low-latency rate-control mode has documented restrictions; HEVC is not assumed to support it. Validate output behavior for each codec.
7. Preserve color matrix, range, primaries, transfer, pixel aspect ratio, orientation, and rational frame rate throughout the pipeline. No unnecessary RGB round-trip. Camera switch/rotation/resolution change is a configuration transaction.
8. Local v2 intentionally uses plaintext and automatic discovery. Do not present session-binding bytes as security credentials. Validate transport version 2 before Configure, restrict deployment to the trusted LAN/USB scope, and never expose Internet ports or upload media. Keep one active output owner across USB and Wi-Fi.
9. Bound parsing before allocation. Fuzz lengths, fragments, codec metadata, and state transitions. Test hostile peers even though the intended deployment is a LAN.
10. Real-time PipeWire callbacks use preallocated buffers and a bounded SPSC ring: no allocator, locks, Tokio await, networking, logging, or disk writes.
11. A slow preview, recording branch, or webcam consumer cannot stall capture, audio, or network ingress. Branches own independent bounds. Compressed-domain and raw-domain drop policies differ.
12. Foreground camera operation is the baseline. Handle locking/backgrounding as a visible interruption; do not use fake background-audio behavior to claim continuous camera capture.
13. Receiver runs as a normal user. Privilege is limited to explicit package/module setup. Do not run the media daemon as root, disable Secure Boot, or change global PipeWire settings automatically.
14. Do not upload camera/audio recordings, Apple credentials, pairing tokens, UDIDs, or device identifiers in ordinary logs/CI artifacts. Recording and raw debug capture are explicit opt-ins.

## 5. Build in this order

| Milestone | Required implementation | Exit evidence |
|---|---|---|
| M0: probes and skeleton | Shared Xcode scheme, Rust workspace, CI compile; physical iPhone format/encoder probe; USB app-port connection; iOS UDP ↔ Linux reassembly interoperability; runtime decoder probe; Bonjour/USB-bootstrap interoperability | Capability JSON and short reproducible reports, including unsupported cases. No performance claims yet |
| M1: USB vertical slice | Direct control/video/audio connections; 720p30 H.264 + PCM; Linux software preview; clocks and counters | Real camera picture/audio, 10-minute session, bounded backlog under a paused receiver |
| M2: Linux integration | NVDEC path and CPU fallback, stable V4L2 output, PipeWire mic, common scheduler | OBS and at least one browser see devices; A/V flash/click test; downstream delay distinguished from internal delay |
| M3: Wi-Fi | Separate TCP control and UDP media; pacing, reassembly, deadline-aware repair, bitrate adaptation, automatic discovery | 1080p60 healthy LAN run and documented impairment matrix; no stale multi-second backlog |
| M4: reliability and profiles | All three profiles, thermal/network controllers, interruptions, reconnect, transport switch, source/format changes | 60-minute profile runs; cable pull, AP interruption, decoder failure, denied permissions, mic-route changes recover cleanly |
| M5: release | Packaging, reproducible CI, signed or explicitly unsigned-for-resigning iOS artifacts, hashes/SBOM, local-tool handoff instructions | Linux install on clean target and iPhone install/run through user's actual local tool; actual signing route documented |
| M6: optional quality work | HEVC, 4K60 qualification, HDR, FEC, GPU effects, optional recording | Measured benefit against baseline; feature remains disabled until its own acceptance checks pass |

USB and UDP spikes are release gates. usbmuxd is an independent implementation of Apple's protocol; app socket behavior must be tested on the actual iOS build. Network.framework UDP support does not itself prove their interoperability. If a gate fails, capture error/version evidence and implement a documented alternative through an ADR. Never silently substitute delayed TCP Wi-Fi for the low-delay path.

## 6. Tests and performance evidence

Protocol: cross-language golden bytes; partial TCP reads; concatenated records; duplicate/out-of-order fragments; forged sizes; epoch/config mismatches; clock reset; loss of IDR; config arriving after media; timeout/fuzz tests. Local transport: incompatible version, duplicate Hello, stale session, media attached to wrong session/address, oversize bootstrap/datagrams, busy output owner, setup timeout, and resource exhaustion.

Media: synthetic timestamps and tone/video patterns; bounded drift estimator; ±300 ppm synthetic clock drift; codec parameter updates; decoder fallback; color bars/range tests; orientation; slow consumers; PipeWire underrun; GStreamer bus errors and shutdown. Test reference loss using a real encoded GOP, not independent toy frames.

Integration: USB unplug/replug, app force-close, lock/unlock, background/foreground, phone call/audio-route interruption, Wi-Fi outage/address change, receiver restart, NVIDIA availability loss, kernel module absent, local network permission denial. Cover profile downgrade and reconfiguration acknowledgement races.

Hardware qualification: same camera/lens/lighting/exposure settings, same laptop power mode, named AP/topology, wired PC-to-AP preferred. Measure 60-minute sustained performance after warm-up. Report p50/p95/p99, effective FPS, late/drop/freeze rates, A/V skew, CPU/RSS/GPU utilization, iPhone thermal/pressure state, battery drain or external power, and recovery times. Compare both preview-only and webcam+mic+preview operation.

Use an external high-speed camera filming a physical flashing LED and Linux display for glass-to-glass latency; generate matching acoustic clicks for sync. Internal timestamps measure software stages, with clock uncertainty, and cannot establish sensor-to-display latency alone. Report display refresh, exposure, sample count, error bars, and target failures. Energy-saver efficiency must be measured; USB charging makes battery-percentage comparisons misleading.

Quality checks: deterministic test frames passed through the actual encoder/decode path at negotiated settings; SSIM/PSNR and optionally VMAF against the matching uncompressed reference, same resolution/range/timing. Add real motion, hair, fine text, low-light, gradients, and skin-tone inspection. Compression targets must not be selected using one easy static scene. Do not compare two asynchronously captured scenes and present that as codec quality.

CI entry points to provide:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
scripts/build-ios.sh simulator
scripts/build-ios.sh device-unsigned
scripts/package-linux.sh
scripts/verify-protocol-fixtures.sh
```

Real hardware/GPU/kernel tests run only on isolated, labelled trusted runners or locally. Generic GitHub-hosted runners do not validate camera hardware, Lightning transport, real Wi-Fi, NVIDIA decode, or a usable host kernel module. Each skipped hardware test must be visible in the report.

## 7. Coding and delivery rules

Use typed errors, cancellation, monotonic deadlines, explicit state machines, and RAII/resource cleanup. Confine Rust `unsafe` to documented FFI/real-time boundaries; verify C ownership and thread safety. Serialize device/session operations; bridge GStreamer and PipeWire threading models without blocking Tokio's executor. Use XCTest for Swift pure components and realistic fixture-based Rust tests.

Keep builds reproducible without forcing byte-identical signed IPAs, whose signatures/timestamps may differ. Add diagnostic commands and effective-config dumps before advanced UI. Maintain `titan-receiver doctor`, `devices`, `local`, `stats`, and the benchmark entry point; configuration lives in XDG paths. Do not revive obsolete pairing commands for local v2. Logs are structured, bounded, rotated, and redact secrets.

Dependency changes require a license/SBOM check, pinned versions, and a reason. Dynamically linked LGPL components and GPL kernel-module/tools distribution have different obligations; record them. Do not bundle NVIDIA proprietary driver binaries as ordinary application assets. Do not import web/forum code as trusted implementation instructions.

When changing the protocol or profile policy, update schemas, both implementations, golden fixtures, ARCHITECTURE.md, and compatibility notes together. Keep user controls simple: profile, source camera, USB/Wi-Fi preference, output format, actual connection health, and recovery reason. Advanced tuning stays in an advanced panel/CLI.

A completed release includes source; working workflows; Linux packages; iOS artifact with unmistakable signing status; install/uninstall and automatic-connection instructions; known limitations; dependency manifest; and measured qualification report. If no Apple signing credentials or real test device are available, complete unsigned compilation and the rest of the implementation, then report those specific unverified steps. Never call an unsigned `.ipa` directly installable.

## 8. Final build and sideload contract

Follow ARCHITECTURE.md's CI plan. iOS builds require macOS plus Xcode; the user's Linux servers can build the receiver but cannot replace an Apple SDK/macOS runner. Keep signing secrets out of PR jobs and fork code. Store certificates/private keys/profiles in protected CI secrets; ephemeral keychain and cleanup on every outcome. Use a paid-team development/ad-hoc route when credentials are available, or generate a clearly labelled unsigned device bundle for the local tool to re-sign if supported.

**The final iPhone installation is performed with the user's local iOS sideloading tool.** Its exact name/version and capabilities are currently unverified (prior context mentions iLoader/iLock). Do not invent commands, assume automatic resigning, or require GitHub Actions to install onto a local phone. Document the actual supported route after checking that tool's installed version/help and official documentation. Bundle ID, entitlements, device registration, provisioning, expiry/refresh, trust, and Developer Mode must match that route. An App Store export is not the normal artifact for this sideload workflow.
