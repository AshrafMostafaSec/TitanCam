# TitanCam 0.2.0 — local sender and Linux receiver

Updated 2026-10-01. Companion contract: [AGENTS.md](AGENTS.md). The owner explicitly requested a simpler trusted-LAN application without authentication, encryption, pairing forms or an iPhone preview. This specification governs implementation; [original research](docs/architecture-original-research.md) retains the earlier sources and experimental design. Transport v2 deliberately supersedes the encrypted alpha.1 application. Install matching new builds on both ends.

## Product and operating model

The foreground iPhone captures and hardware-encodes video/audio, then sends them. Its screen has connection status, a USB button, automatically discovered computer buttons, refresh, and stop. There is no camera preview, QR, token field, certificate or account. Capture stops on backgrounding/locking, but **not** when an iOS permission dialog temporarily makes the scene inactive.

The Linux desktop application starts the receiver immediately, publishes Bonjour, monitors attached phones and controls the profile, optional desktop preview and virtual webcam. Closing its window leaves the receiver running; the explicit Stop/Exit controls stop it. An optional user autostart entry starts it at login. One active session owns GStreamer/PipeWire outputs across both transports. USB and Wi-Fi are alternatives; simultaneous duplicate pipelines are rejected. Changing profile/output settings applies through an orderly receiver restart and sender reconnect.

The application has no Internet port forwarding, accounts, analytics or camera uploads. Local v2 media and control are **unencrypted and unauthenticated by explicit product choice**. Session IDs/binding bytes prevent accidental cross-session routing; they do not provide security against a hostile LAN. Wi-Fi deployment uses the owner's trusted LAN. Apple device Trust and camera/microphone/local-network permissions still apply.

## Data path

- iPhone: AVFoundation → timestamped NV12 → VideoToolbox hardware H.264/HEVC → Annex-B access units; audio uses one normalized capture clock, 48 kHz PCM on USB or Opus on Wi-Fi.
- USB: four app-owned TCP listeners reached through libusbmuxd/usbmuxd; no Lightning accessory SDK is implied.
- Wi-Fi: TCP 49160 for bounded framed control; UDP 49161 for paced fragmented video/audio. Bonjour `_titancam._tcp` discovers the receiver. QUIC was removed because it requires TLS.
- Linux: Tokio framing/reassembly → bounded media queue → one GStreamer NVDEC/CPU decoder → optional preview and v4l2loopback. Audio → Opus/PCM → bounded native PipeWire source.

Use existing module boundaries: `titan-protocol` is pure binary/config parsing; `titan-transport` contains bounded record I/O and random IDs; `titan-usb` owns the narrow C shim/FD adapter; `titan-media` owns GStreamer and real-time PipeWire; `titan-receiver` owns lifecycle/control/discovery; `titan-bench` generates a real encoded GOP and intentionally loses a reference frame. No crypto library or TLS certificate/Keychain setup is needed in current code.

## iOS capture and hardware encoding

Target iPhone 13, iOS 17.4+, Swift/SwiftUI and AVFoundation/CoreMedia/CoreVideo/VideoToolbox/Network.framework. Camera hardware is qualified on the physical phone, not a simulator. A serialized capture owner performs format changes/start/stop away from the main thread. Capture callbacks enqueue owned timestamped buffers and never block on networking.

Require hardware VideoToolbox encoding and verify the effective session. Low-delay profiles disable B-frame reordering and keep encoder backlog bounded. Preserve NV12, color/range metadata, orientation and rational timing; avoid an RGB preview branch on the phone. Report effective resolution/FPS/codec when a device format cannot satisfy the request. H.264 is the initial interoperability baseline; maximum/HEVC remain experimental until sustained hardware testing.

All samples are normalized against the host capture clock. Audio route/interruption, backgrounding, thermal critical state and encoder errors end/reconfigure the session visibly. Serious thermal pressure reduces FPS/bitrate/resolution. Network expiry lowers bitrate; stable feedback slowly restores the requested budget. No fake background-audio workaround is used.

## Discovery and direct connection

Linux launches `avahi-publish-service --no-fail` with a human-readable host name and publishes these bounded public TXT fields:

```text
v=2
mode=plain
host=<current LAN IPv4>
media=49161
```

The service port is 49160. No credentials or media appear in TXT. The desktop launcher observes LAN address changes and restarts the receiver/announcement on the new address. It also works without a LAN for USB. Avahi's daemon and utilities are packaged dependencies. The phone uses `NWBrowser(.bonjourWithTXTRecord(...))`, with `NSBonjourServices=[_titancam._tcp]` and an accurate local-network permission description. Only v2/plain advertisements with valid bounded address/ports produce a Connect button. IPv6-only and multiple-interface selection are future extensions; initial desktop routing follows the active IPv4 default interface.

Allow TCP 49160, UDP 49161 and UDP 5353 only on the intended LAN interface/subnet. Do not disable the firewall globally. AP/client isolation or denied iOS Local Network permission can still prevent discovery/connectivity. A discovered service is a network announcement, not proof of authenticated identity.

## USB listener lifecycle and bootstrap

Default base 43052; fallback bases 43062/43072. For each base:

| Role | Port | Framing |
|---|---:|---|
| Metadata | base−1 | One ≤256-byte JSON document, then EOF |
| Control | base | 4-byte BE length + bounded JSON |
| Video | base+1 | 4-byte BE length + 64-byte TCAM header + complete access unit |
| Audio | base+2 | Same media record, complete PCM unit |

Metadata is exactly `{ "version": 2, "base": 43052 }` for the corresponding base. Linux probes known bootstrap ports through usbmuxd, bounds the response before allocation, checks version/base, then opens the three data channels automatically. No pin/token entry is required. Polling is quiet while the phone is absent or has not enabled USB; enumeration runs off the async executor and device IDs never appear in ordinary logs.

USB-ready means all four listeners reached `.ready`. Repeated taps reuse that set. `NWListener.cancel()` is asynchronous: replacement/rebind waits for every previous `.cancelled` callback. A generation counter rejects stale accepts/configuration callbacks. EADDRINUSE cancels the entire set and tries the next base; never create partially mixed channel sets. Limit pending bootstrap/media connections and expire unfinished binding. Stop, USB→Wi-Fi and immediate restart all use the same cancellation barrier. Physical usbmux reachability must still be exercised on the installed phone build.

## Control handshake and start barriers

Control retains the v1 JSON envelope and 64-byte media header for byte compatibility of framing; the **connection mode is v2** and is negotiated before configuration. An older encrypted app cannot connect to this plain transport.

1. Receiver creates random 16-byte session ID and automatic 32-byte media-routing binding value. Send `Hello` with `transport_version:2`, `mode:plain`, `media_token`, `media_port:49161`.
2. Phone validates mode/version/session/bounds and sends `HelloAck` with `transport_version:2`. No AuthProof, certificate, signature or user token exists.
3. Receiver sends requested `Configure`; phone serializes AVFoundation/encoder changes and replies effective `ConfigureAck`. Receiver validates it. Heartbeats during slow configuration are accepted; setup is bounded to 30 seconds. Hello is bounded to 5 seconds.
4. `MediaReady` permits media channel setup. USB video/audio send `MediaBind` with role/session/binding; Wi-Fi sends the existing 57-byte `TCMB` binding datagram. These values are automatic routing state, not authentication.
5. Wi-Fi binds one UDP source address to the TCP peer IP and current session. Only then request `MediaBound` and `Start`. Retransmitted binding gets acknowledgement without duplicate Start. USB requests Start after both media writes; the phone waits until both roles are attached.
6. Phone sends `StartAck` with its capture clock epoch. Receiver marks the session live and sends `StreamingReady`. Phone starts capture after this final barrier.

One output semaphore is retained by the media worker until all video/audio resources have been released, including cancellation paths. Reject a second session while the first owns outputs. Four concurrent incomplete control handshakes are allowed; parsing remains bounded. Heartbeats run every 500 ms; control watchdog is 3 seconds. Session leases set cancelled/live state on failure and queued resources drain within their bounds.

## UDP pacing, loss and recovery

Each datagram is ≤1100 bytes: a 64-byte TCAM header plus ≤1036 payload bytes. Fragment fields retain original access-unit length, index/count/offset, session, transport epoch and configuration ID. Validate lengths/overlaps before allocation. Reassembler rejects stale sessions/configs, handles duplicate/out-of-order fragments, and expires incomplete units after 60 ms. No unbounded retransmission or delayed TCP-media fallback is used.

The sender prioritizes audio, spaces video fragments with a bitrate token bucket, bounds outstanding send completions to eight, queues at most two video units and ten audio units, and abandons stale video after 100 ms. Dropping dependent video triggers a fresh IDR. Feedback every 100 ms reports actual accepted frames, expiry, drops, decoder, queue and clock statistics. Losing a reference frame requires decoder flush/wait-IDR recovery; do not continue corrupt dependent pictures.

Plain UDP has no QUIC congestion controller: the app's pacing, feedback-driven bitrate reduction and bounded deadlines are the current LAN controls. There is no promise to deliver every live frame. Selective repair/FEC and calibrated congestion/impairment matrices remain further qualification work. USB uses reliable TCP but closes a stale/overrun media channel to restore freshness.

## NVIDIA efficiency and Linux outputs

Prefer RTX 4050 Laptop **NVDEC**, the dedicated video decoder, rather than a general CUDA inference/shader workload or re-encoding. Decode once and tee to independently bounded branches. `nvh264dec`/`nvh265dec` use `max-display-delay=0`, automatic output surface count and corrupt-frame discard. Software fallback remains available and is reported in actual statistics.

Desktop preview uses `glimagesink` when available with NVIDIA, allowing GL-memory negotiation for preview-only operation. Its two-buffer leaky queue and bounded lateness prevent a slow display from blocking ingress. With a virtual webcam, a system-memory NV12 output may require a GPU→CPU download; this is not claimed to be universal zero-copy. The webcam keeps native negotiated dimensions and NV12, removing forced 1080p upscale/downscale and YUY2 conversion. No decode duplication or enhancement/sharpen/denoise shader is added. Installed CUDA scaling elements are feature-probed; unavailable elements are not assumed present.

NVDEC does not guarantee a fixed low GPU percentage: workload, display refresh, resolution, other programs and driver affect utilization. Efficient saver/balanced defaults and optional preview reduce load; do not set global GPU clocks or a power cap that affects unrelated applications. Qualification records decode utilization, total GPU use/power, CPU/RSS, effective FPS and output branch behavior. [GStreamer NVDEC](https://gstreamer.freedesktop.org/documentation/nvcodec/nvh264dec.html).

Virtual webcam uses a separately installed v4l2loopback DKMS module and an accessible device such as `/dev/video42`; the media receiver remains unprivileged. Do not disable Secure Boot or alter unrelated cameras. Native PipeWire Audio/Source uses preallocated SPSC storage; the real-time callback performs no allocations, locks, logging or async work. Queue/drop/underrun counters are visible. Opus decoder handles packet loss; a small bounded resampling correction manages queue drift.

## Profiles and measurable qualification

| Profile | Requested starting video | Purpose |
|---|---|---|
| Saver / best efficiency | 720p30 H.264 | Lowest default load; first USB qualification |
| Balanced | 1080p60 H.264 | Smooth motion with bounded queues and adaptive bitrate |
| Maximum | Up to 4K60 / HEVC | Experimental quality target subject to format, thermal and bandwidth support |

Authoritative bitrate/audio/playout starting values live in `StreamConfig::profile` and `config/profiles/`; keep them synchronized. Effective camera settings can differ and must be reported. No sustained 4K60 guarantee or better-than-sensor claim exists. Compression is not lossless.

Initial **unverified glass-to-glass goals** at 1080p60 with good lighting, low-load host, 60+ Hz display: USB p50 45–80 ms / p95 ≤120 ms; healthy local Wi-Fi p50 60–110 ms / p95 ≤160 ms. Saver 30 FPS naturally adds frame-period delay. These are engineering acceptance targets, not measured results or literal zero latency. Measure capture/encode/transport/decode/display separately and report downstream OBS/browser delay separately. Internal clock RTT is not glass-to-glass latency.

Qualify ten-minute USB and Wi-Fi sessions, then 60-minute profile runs; report p50/p95/p99, effective FPS, freeze/drop/late rates, A/V skew, CPU/RSS/GPU decode/power, phone thermal state and battery/charging context. Use a filmed physical flashing LED plus corresponding acoustic click, with exposure/refresh/error bars documented. Test cable pull, phone background, restart, AP outage, denied permissions, source/output failure and busy-session races. Target A/V skew p95 ≤30 ms under healthy conditions; investigate drift over sustained runs. Synthetic protocol/media tests do not establish these hardware targets.

## Builds, packaging and installation

Linux GitHub Actions matrix builds/tests/packages Ubuntu 24.04 and 26.04 x86_64 separately; their usbmuxd ABIs differ. Run fmt/clippy/protocol/transport/lifecycle tests, cross-language golden fixture, and a generated H.264 TCP+UDP recovery/configuration benchmark. Include locked dependency manifest, notices, hashes and matching Debian/tar artifacts. NVIDIA drivers and DKMS module are separate host prerequisites.

iOS builds on GitHub-hosted macOS 15 + Xcode 16.4 using pinned XcodeGen and Opus build inputs. Linux does not emulate Apple's SDK. Compile a real-device bundle and run simulator parsing/discovery/plain-USB handshake/listener lifecycle tests. The iOS application no longer needs Swift certificate/ASN1/crypto packages. Device compilation does not prove physical camera/mux connectivity.

Default distribution is an explicitly **unsigned IPA for local re-signing**. The user's local iLoader performs final signing/provisioning/install with Apple credentials remaining local. Valid Apple signing, compatible bundle ID/entitlements, provisioning, device trust, expiry and Developer Mode where required still apply. If a signed Actions route is later requested, use protected certificate/profile secrets, ephemeral keychain, cleanup, and never expose secrets to PR/fork code. Current workflow requires no Apple secrets and does not install onto the user's phone automatically.

Publish only a development prerelease until physical acceptance passes. Verify release asset hashes, successful exact-source builds, correct Ubuntu ABI, installed binary/GUI version, package dependencies and live Pages link. Do not upload phone identifiers, camera/audio, credentials or local diagnostic secrets. See [build/signing](docs/BUILD_AND_SIGNING.md), [status](docs/NEXT_STEPS.md) and [README](README.md).
