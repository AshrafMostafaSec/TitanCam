# TitanCam architecture and implementation specification

Version: 1.0 planning baseline. Research checked **2026-09-30**. Status: researched design, **not a built or benchmarked product**. Audience: implementing AI coding agent and human reviewer. Companion: [AGENTS.md](AGENTS.md).

## 1. Scope, evidence, and decisions

Build an iPhone 13 → Ubuntu/Linux professional camera/audio link with a direct preview, virtual webcam, and virtual microphone. Optimize for short and stable capture-to-output delay, image fidelity, clean audio, recoverability, and sustainable device power. Operate over Lightning USB or a local Wi-Fi LAN. Internet relays, NAT traversal, browser-direct ingest, multi-camera capture, and background camera service are outside v1.

Apple lists iPhone 13 video recording up to 4K60, H.264/HEVC, stereo recording, and Wi-Fi 6. This establishes a device ceiling, not a guarantee of arbitrary AVFoundation formats, stereo routes, simultaneous processing, or continuous 4K60 streaming. Discover the actual capture formats and encoder capabilities on the installed OS. Do not infer Wi-Fi 6E/6 GHz support, ProRes capture, AV1 encoding, or a raw Bayer video feed. [Apple device specifications](https://support.apple.com/en-us/111872)

The selected architecture is an original engineering design based on the API evidence below. Numerical budgets, controller thresholds, wire formats, and profile bitrates are proposed starting settings. Confirm them in implementation and qualification reports.

| Decision | Rationale and limitation |
|---|---|
| Native Swift capture + VideoToolbox | Direct control of formats, timestamps and hardware encoding; avoids an unnecessary platform wrapper |
| Rust receiver + GStreamer | Rust handles policy/networking; existing media libraries parse/decode/convert video; do not write a decoder |
| H.264 first; measured HEVC upgrade | Dedicated Apple low-latency rate-control documentation specifically covers H.264. HEVC may improve quality per bit but requires independent latency/power validation |
| USB: app TCP sockets through libusbmuxd | Stream forwarding through usbmuxd, not USB Video Class, not raw USB bulk access from a sandboxed app, not Personal Hotspot IP networking |
| Wi-Fi: QUIC DATAGRAM media + separate TLS/TCP control | QUIC provides encrypted congestion-controlled unreliable datagrams; separate control simplifies native Apple interop and keeps reliable commands distinct from expired media |
| No general media retransmission promise | Late pictures are less useful than fresh pictures. Repair only within deadlines; retain a reliable control/config path |
| NVDEC preferred, CPU fallback | RTX 4050 Laptop supports relevant H.264/HEVC decode families; driver/plugin/runtime caps still gate actual use |
| V4L2 camera + native PipeWire mic | Practical compatibility with existing Linux applications; does not guarantee downstream clients preserve negotiated source quality or sync |
| SDR default | Broad consumer compatibility; HDR is optional and requires end-to-end metadata and conversion support |

Alternatives: WebRTC is appropriate for browser connectivity, ICE/STUN/TURN and standardized RTP feedback, but adds an SDK/capture-encoder integration surface and codec negotiation complexity. SRT/reliable streaming can suit recording/contribution links, but retransmission and playout buffering trade against this design's local low-delay deadline. Raw NV12 at 1080p60 is roughly 1.49 Gbit/s before overhead; 4K60 is roughly 5.97 Gbit/s, so compressed transport is essential. Do not add all transports to v1.

## 2. End-to-end data path

```text
iPhone main/front camera ─ AVFoundation ─ CVPixelBuffer NV12 ─ VideoToolbox HW encoder
                                 │                               │
                             capture PTS                  AU + codec config
                                 │                               │
Microphone ─ capture audio PCM ─ timestamp/packetizer ─────────────┤
                      └ Wi-Fi Opus / USB PCM                      │
                                      bounded transport scheduler
                                         │                  │
                              USB TCP/TLS via usbmuxd    Wi-Fi QUIC DATAGRAM
                                         │                  │
                                Rust validate/reassemble/session-clock
                                              │
                                   video GStreamer appsrc
                                              │
                                 parse ─ NVDEC/CPU decode
                                              │
                                  common presentation policy
                            ┌─────────────────┼──────────────────┐
                       GPU preview       V4L2 conversion     optional recording
                                         + v4l2loopback

audio ─ decode/PLC ─ adaptive resample ─ timestamped ring ─ PipeWire virtual mic
```

One session coordinator owns negotiation, authentication, clocks, lifecycle, controller decisions, and effective configuration. Module boundaries exchange typed timestamped units, not raw pointers with implicit lifetimes.

Linux is the session coordinator regardless of which endpoint accepts the underlying socket. It creates session IDs, proposes configurations after phone capability discovery, and commits transport epochs. The phone may refuse/downgrade a proposal for unsupported hardware or thermal safety. Control transactions have unique IDs, idempotent responses, a 2-second initial timeout and at most two retries; timed-out configuration is never treated as applied. Limit JSON nesting to 16 and capability lists to 256 entries per category; paginate exhaustive diagnostics separately. Codec parameter sets in control use base64 fields with decoded-size bounds; negotiated configuration must still fit the 64 KiB record limit.

Data types: `CapturedVideo(buffer, capturePTS, duration, formatID)`, `EncodedAU(bytes, pts, duration, independent, configID, epoch)`, `AudioUnit(bytes, firstSamplePTS, sampleCount, codec)`, `SessionConfig`, `TransportFeedback`, `EffectiveProfile`, and `PresentationEvent`. Use rational frame durations and integer nanoseconds for the protocol.

## 3. iOS capture implementation

### 3.1 Session and format selection

Use `AVCaptureSession`, one `AVCaptureDeviceInput` for the selected camera, `AVCaptureVideoDataOutput`, and `AVCaptureAudioDataOutput` initially. Keep all capture-session topology/configuration work on one serial queue or isolated actor backed by an appropriate executor. Start/stop off the main thread. Delegate queues are separate serial queues for video and audio.

Enumerate `AVCaptureDevice.formats`, dimensions, subtype, color spaces, supported frame-rate ranges and HDR support. Select a matching format; under `lockForConfiguration`, set `activeFormat` and equal min/max frame durations for a fixed requested rate. Setting an active format changes session preset behavior; do not later overwrite it with a competing preset. [Apple format configuration](https://developer.apple.com/documentation/avfoundation/capture-device-formats)

Use a single physical wide camera initially to avoid unannounced lens switching. User camera changes are explicit transactions. Supported exposure/focus/white-balance controls are capability-gated. Use automatic controls initially, with optional lock after settling. Auto-exposure can increase capture delay and blur in poor lighting; high FPS cannot replace good lighting. Electronic stabilization is off in latency profiles by default; benchmark it separately. Do not promise control over every ISP/OIS behavior.

Request an encoder-compatible bi-planar 4:2:0 format from the output's supported pixel types, typically NV12 video range. Preserve the actual range rather than assuming full range. Avoid BGRA conversion unless a consumer genuinely requires it. Record color attachments and propagate them. [Apple pixel-format guidance](https://developer.apple.com/documentation/technotes/tn3121-selecting-a-pixel-format-for-an-avcapturevideodataoutput)

Set `alwaysDiscardsLateVideoFrames = true`; implement the dropped-sample delegate and reason counters. Capture callbacks only timestamp, retain briefly under a strict ownership budget, and enqueue to encoder. Start with two in-flight video buffers maximum, reducing FPS when pressure persists. [Apple dropped-frame guidance](https://developer.apple.com/library/archive/technotes/tn2445/_index.html)

Use `session.masterClock`/CoreMedia clock conversion to map captured audio and video PTS to the same host-time domain. Establish a capture epoch and normalize PTS to unsigned nanoseconds since that epoch. Detect discontinuities explicitly. Never substitute callback-entry time for sample PTS; record both for instrumentation.

### 3.2 Audio

Let the app own audio-session configuration (`automaticallyConfiguresApplicationAudioSession = false` where applicable). Baseline `AVAudioSession` is a capture-oriented category with `.measurement` mode when supported, preferred 48 kHz, built-in microphone, and requested 5–10 ms I/O duration. Query actual category, route, channels, sample rate, input latency and I/O duration after activation. Preferences are hints. Audio output monitoring, voice processing or route changes may require a different category/mode and a new effective configuration. [Apple buffer-duration API](https://developer.apple.com/documentation/avfaudio/avaudiosession/setpreferrediobufferduration(_:)), [audio-session preference guidance](https://developer.apple.com/library/archive/qa/qa1631/_index.html)

Packetize using a preallocated audio ring outside the callback. Convert/resample actual input once to 48 kHz when necessary. Default mono, optional actual supported stereo input; two duplicated mono channels are not stereo capture. Keep the built-in microphone as the qualification route; Bluetooth changes latency/quality and must be reported. On route/interruption changes, drain/drop stale audio, create a new audio configuration/epoch, and renegotiate.

USB PCM: S16LE interleaved, 48 kHz, 5 ms packets (240 samples/channel), mono default or stereo maximum mode. Payload is 480/960 bytes; 0.768/1.536 Mbit/s excluding overhead. Wi-Fi Opus: libopus built as a pinned static XCFramework for device and simulator architectures, invoked via a small Swift/C boundary; no unsupported assumption that AudioConverter supplies Opus encoding.

Opus starts with 10 ms frames for balanced, 20 ms saver, and 5 ms maximum low-delay mode. Query codec lookahead and include it in timestamp alignment. Use PLC on missing packets. `RESTRICTED_LOWDELAY` removes SILK operation; do not combine it with a promise of SILK in-band FEC. Optional FEC uses compatible modes/frame durations and waits for the subsequent packet, spending an extra packet interval. Audio/music and voice modes must be separately negotiated. [Opus API values](https://www.opus-codec.org/docs/opus_api-1.6/group__opus__ctlvalues.html), [encoder controls](https://www.opus-codec.org/docs/html_api-1.0.2/group__opus__encoderctls.html), [upstream implementation](https://github.com/xiph/opus)

## 4. VideoToolbox encoding and fidelity

Create `VTCompressionSession` for the negotiated codec/dimensions. Request hardware encoding and confirm `UsingHardwareAcceleratedVideoEncoder`; hardware failure triggers an explicit downgrade/error, not an invisible high-power software encoder. Inspect supported property dictionaries and every `OSStatus`.

Set supported `RealTime = true`, `AllowFrameReordering = false`, `ExpectedFrameRate`, `AverageBitRate`, profile/level, and supported rate limits. Probe `MaxFrameDelayCount`; it is an optional tuning control, not a portable guarantee. `DataRateLimits` is a byte/window constraint, not a network pacer. Set color metadata. Never force unsupported profile/level combinations. Encoder callback latency and emitted dependencies are runtime evidence. [Apple compression property catalog](https://developer.apple.com/documentation/videotoolbox/compression-properties)

For H.264, first attempt `kVTVideoEncoderSpecification_EnableLowLatencyRateControl` at session creation. Apple documents H.264 hardware operation and restrictions including no reordering/lookahead, High profile and an infinite-GOP structure. Do not configure Baseline and silently assume the flag succeeds. Treat periodic recovery IDRs as explicit requests and validate the emitted independent frame; if the chosen encoder cannot satisfy recovery, use a documented bounded-GOP fallback. [Apple low-delay encoding session](https://developer.apple.com/videos/play/wwdc2021/10158/), [low-latency mode key](https://developer.apple.com/documentation/videotoolbox/kvtvideoencoderspecification_enablelowlatencyratecontrol)

For HEVC, use normal supported real-time/no-reordering settings and measure them. Do not assume the H.264-only low-latency key works for HEVC. Choose HEVC only if both ends pass negotiation, NVDEC/CPU probes and a latency/energy/quality comparison. SDR HEVC Main is the first HEVC target; Main10/HDR is separate.

Use CMSampleBuffer format descriptions to extract SPS/PPS, or VPS/SPS/PPS for HEVC. Normalize compressed access units to Annex B for the v1 protocol, carefully converting length-prefixed NALs with validated length fields; never assume CMSampleBuffer bytes already contain Annex B start codes. Obtain independent-frame status from attachments plus bitstream validation. PTS equals capture PTS; DTS equals PTS only after proving no reorder. Send codec configuration reliably before the first unit using it, and include required parameter sets with recovery IDRs.

Initial IDR policy: one at start/configuration/reconnect, recovery on demand, and a periodic 1-second Wi-Fi / 2-second USB refresh when supported. Rate-limit recovery requests to one per 250 ms. IDRs have bandwidth bursts; pace fragments and reserve capacity. If an IDR cannot meet the fresh-frame budget, lower resolution/bitrate and retry rather than queueing megabytes for seconds. Long-term reference/temporal layers are later experiments, not a prerequisite for v1.

Pass source CVPixelBuffers directly to VideoToolbox. Release resources after completion; no encode flush on each frame. A bounded pool is mandatory if filters create new buffers. Preview may be disabled or reduced in saver mode without changing capture semantics. Any filters (denoise/sharpen/tone map/LUT) default off and are benchmarked separately; they can improve subjective presentation while adding delay, energy and artifacts.

SDR default: 8-bit 4:2:0, source primaries/transfer/matrix/range recorded. Do not claim equality with Apple's stock Camera app processing. Preserve source resolution when available, and report any downstream scale. Optional HDR uses a supported capture format, 10-bit pixel path, HEVC Main10, explicit color metadata and a qualified display; it does not imply reproducing Dolby Vision metadata. Webcam SDR requires deliberate tone mapping, never merely truncating 10-bit values.

## 5. USB transport

Linux subscribes to libusbmuxd device events and selects an explicit device UDID (redacted in ordinary logs). It opens app listening ports through `usbmuxd_connect`; an `iproxy` experiment is useful during M0, but production uses the library directly. Wrap returned descriptors with correct close/ownership semantics, partial-read handling, and nonblocking integration. usbmuxd is a daemon socket service; libusbmuxd is the client library. [libusbmuxd](https://github.com/libimobiledevice/libusbmuxd), [usbmuxd](https://github.com/libimobiledevice/usbmuxd)

The foreground iOS app exposes three TCP/TLS listeners, default ports **43052 control, 43053 video, 43054 audio** (fallback bases 43062 and 43072 if occupied; alpha.1 used legacy base 49152). Linux is the TCP/TLS client and iPhone is TLS server. The phone reports the effective base port; Linux selects it with `--usb-base-port`. USB-ready is reported only after all three listeners reach `.ready`. Repeated Enable taps renew the pairing token without rebinding; stop/restart and transport changes wait for `.cancelled` callbacks, and stale callbacks are rejected by generation. Listener parameters permit local endpoint reuse; occupied triplets are cancelled fully before trying another. See `protocol/usb-ports-v1.json` and the listener lifecycle simulator tests. [Apple listener port lifecycle](https://developer.apple.com/documentation/network/nwlistener/port), [endpoint reuse](https://developer.apple.com/documentation/network/nwparameters/allowlocalendpointreuse). Prove on the physical iPhone that app-owned listening sockets can be reached through usbmuxd and that the chosen listener binding is appropriately constrained. Do not assume a localhost-only bind works on every iOS version. If a broader bind is necessary, enforce TLS/auth before all commands/media and test unwanted LAN access; USB media channels require a control-issued single-use binding token.

This does not make the iPhone a UVC device and does not require a jailbreak or an SSH daemon. Normal device trust/unlock behavior and usbmuxd permissions must be tested and explained. No entitlement provides direct raw Lightning access for this design. Do not expose the usbmuxd daemon socket over a network.

Each connection is TLS 1.3 authenticated at application level; media uses framed complete-unit records. Separate audio/video/control sockets prevent a large video record from blocking audio/control at the application layer, but all still share the USB link and can contend. Configure TCP_NODELAY where supported and justified; do not assume control over usbmuxd's internal socket options or throughput. Measure effective goodput rather than using advertised cable speeds.

TCP cannot cancel bytes already queued in the kernel/daemon or a partially transmitted record. Sender queues accept at most two complete AUs and a profile age limit; receiver tracks capture age. If a video connection becomes stale, close it, open a new connection/transport epoch, send configuration and an IDR, and resume. Epochs are session-wide: commit the new epoch over authenticated control and update all active media channels at the agreed boundary, including audio, before accepting its data. Existing audio sockets may remain connected but cannot continue sending the old epoch after the commit. Never splice a half-sent length-prefixed record into another record. A paused/overloaded receiver must not create an unbounded sender memory queue.

## 6. Wi-Fi transport and network controller

### 6.1 Discovery and sockets

Preferred topology: iPhone on a strong 5 GHz Wi-Fi 6 AP; Ubuntu PC wired to the same AP. The iPhone 13 supports Wi-Fi 6, not an assumed 6 GHz radio. Wi-Fi-to-Wi-Fi through one AP spends additional airtime; report it separately. Avoid universal channel-width prescriptions; interference and router capabilities determine useful settings.

Receiver publishes `_titancam._tcp` control discovery on **49160/TCP** and includes media port **49161/UDP** plus protocol version in non-secret TXT records. iOS uses Bonjour/NWBrowser and Network.framework. Supply `NSCameraUsageDescription`, `NSMicrophoneUsageDescription`, `NSLocalNetworkUsageDescription`, and `NSBonjourServices = ["_titancam._tcp"]`. Bonjour APIs do not mean implementing custom raw multicast discovery; only request multicast entitlement if the actual operations require it. Permission denial is a first-class UI state. Support QR/manual address for networks blocking discovery. [Apple local-network privacy](https://developer.apple.com/documentation/technotes/tn3179-understanding-local-network-privacy)

Wi-Fi roles: Linux TLS control server and QUIC media server; iPhone connects as client. Control is TLS 1.3 with ALPN `titancam-control/1`. Media is raw QUIC v1, ALPN `titancam-media/1`, RFC 9221 DATAGRAM enabled. It is **not HTTP/3 or WebTransport**. iOS starts a datagram-only `NWProtocolQUIC` flow using `isDatagram` and `maxDatagramFrameSize`; Linux uses Quinn. Keep reliable control on its own TLS socket, avoiding a dependency on mixing incoming streams and extracted datagram flows in the Apple API. [Apple QUIC options](https://developer.apple.com/documentation/network/nwprotocolquic/options), [Apple datagram introduction](https://developer.apple.com/videos/play/wwdc2022/10078/), [Quinn connection API](https://docs.rs/quinn/latest/quinn/struct.Connection.html)

M0 must exchange datagrams in both directions, validate ALPN/cert pins, inspect negotiated size, and prove reconnect on the actual iOS version. Different vendor APIs can disagree despite standards support. If native QUIC interop fails, a documented adapter using a maintained QUIC implementation via C/Swift FFI is the preferred investigation; account for arm64 device/simulator builds and library footprint. A TLS/TCP media fallback may be explicitly offered as a degraded mode with different latency behavior, not reported as meeting healthy-QUIC targets.

### 6.2 Pacing, reassembly, and loss

QUIC datagrams are unreliable, unordered, not retransmitted by QUIC and still subject to congestion control. Application fragmentation is required for large AUs; do not rely on IP fragmentation. Begin with **1100 bytes total application datagram**, further limited by negotiated sender/receiver maximums; use 64 bytes for the v1 header. Reject sessions unable to support a useful minimum (512 bytes proposed). Changes in MTU require smaller future fragments. [RFC 9221](https://www.rfc-editor.org/rfc/rfc9221)

Sender schedules audio and control feedback before video fragments; fairly pace video over its frame interval. Avoid an entire IDR being submitted to an opaque framework queue in one burst. Bound outstanding send completions, datagram bytes, fragment count and age; reset the media connection when stale internal backlog cannot be removed. On Quinn use fresh-data send semantics and bounded send buffers rather than waiting indefinitely for old media to fit. Maintain a bounded AU repair cache (250 ms and 16 MiB maximum).

Receiver assembles complete video AUs using session/epoch/config/sequence and validates fragment offsets. Incomplete units expire at a profile deadline. If any required reference AU is missing, stop submitting dependent video, flush/reset as needed, and request an independent picture. Default v1 assumes every inter-coded AU may be a reference. Reassembled audio is normally one datagram; missing units receive PLC/short silence rather than indefinite waiting.

Feedback at 100 ms includes last received/complete sequences, missing-fragment ranges (bounded), packet loss/reorder/late counts, estimated jitter, RTT, queue age, decode/output lateness and effective FPS. ACK/NACK is an application observation, not a replacement for QUIC ACK/congestion control. NACK only when predicted repair arrival precedes the AU presentation deadline by at least 5 ms. Try one selective repair at most; otherwise request IDR. Limit repair traffic to 10% of media budget, and never NACK unknown/expired epochs. Protect audio capacity.

Optional video FEC is disabled until justified by impairment measurements. A future negotiated XOR-parity group of up to eight fragments can recover one missing fragment, with exact lengths and group descriptors in an extended protocol version; multiple losses still require recovery. Parity/duplicates count against the same bandwidth budget. Do not invent ad-hoc parity bytes inside v1.

### 6.3 Adaptation

QUIC's congestion controller remains responsible for network safety; application bitrate adaptation manages camera/encoder production and queue latency above it. Do not disable congestion control or run an independent pacer that ignores QUIC backpressure. [RFC 9002](https://www.rfc-editor.org/rfc/rfc9002.html)

Starting policy, evaluated every 100 ms with 1-second statistics windows:

- Compute safe media production budget from observed goodput, sustained send-backpressure and feedback; start conservatively and reserve 20–30% for overhead, IDRs, audio and repair. Delivery rate while application-limited is not the link's capacity. No capacity claim from one short speed test.
- If oldest unsent/received age exceeds 20 ms or loss exceeds 2% for two windows, reduce target video bitrate by 20%, within the profile floor. If loss exceeds 5% or age exceeds 50 ms, cut by 40%, drop stale work and initiate reference recovery.
- If stable for 5 seconds with low loss (<0.5%) and queue age (<10 ms), increase bitrate by at most 5% per second to the profile ceiling. A bitrate change must be applied/read back before claiming success.
- Persistent congestion for 3 seconds moves down the profile's resolution/FPS ladder; upgrade only after 15 stable seconds, one step at a time. Record reason and all transitions. Lower bitrate first for network pressure; lower FPS/work first for thermal pressure.
- Jitter target follows a robust high percentile of positive transit variation, with profile clamps and slow adjustment. Default stays small; beyond the clamp reduce the profile or warn rather than silently accumulating delay.

Thresholds are tuning values. Test loss, competing traffic, burst loss, bufferbloat and CPU overload separately; not every dropped frame is a network problem. iOS does not expose dependable AP RSSI/channel or disable-Wi-Fi-power-save controls to an ordinary app; do not invent them. Linux may display available link metadata, with missing values labelled unknown.

## 7. Protocol v1, control contracts, and security

### 7.1 Framing

Control records: 4-byte unsigned big-endian payload length, followed by UTF-8 JSON, max **64 KiB**. No newline dependency. Required envelope: `version=1`, `type`, `request_id` (u64 represented as decimal string in JSON), `session_id` (16 random bytes as hex), `transport_epoch` (u32), `body`. Unknown optional fields may be ignored; unsupported required features/major versions fail negotiation. Pure Swift/Rust parsers share schemas and golden fixtures.

Required messages: `Hello`, `Capabilities`, `AuthChallenge`, `AuthProof`, `PairRequest`, `PairAccept`, `Configure`, `ConfigureAck`, `Start`, `StartAck`, `Stop`, `Feedback`, `RequestIDR`, `ClockPing`, `ClockPong`, `Heartbeat`, `Error`, `MediaBind`, `MediaBound`, `TransportSwitch`, `TransportReady`. Prior to authentication, only a tightly limited hello/auth/pair subset is allowed.

`Capabilities` includes camera formats/rates/pixel types; codecs/profiles/bit depth; hardware status; audio routes/rates/channels; transport sizes; decoder caps; output caps; and protocol optional features. `Configure` names `config_id`, capture choice, rational FPS, codec/profile/bitrate, color fields, audio parameters, profile ID, output dimensions/FPS, delay budget, and epoch. `ConfigureAck` supplies effective values or a typed rejection. Sender must not emit media using an unacknowledged config.

Wire media header is exactly **64 bytes**, unsigned fields in network byte order:

| Offset | Bytes | Field |
|---:|---:|---|
| 0 | 4 | Magic ASCII `TCAM` |
| 4 | 1 | Version = 1 |
| 5 | 1 | Kind: 1 video AU, 2 PCM audio, 3 Opus audio |
| 6 | 2 | Flags: bit 0 independent video, bit 1 discontinuity; all other bits zero |
| 8 | 16 | Session ID |
| 24 | 4 | Transport epoch |
| 28 | 4 | Config ID |
| 32 | 8 | Unit sequence, monotonic per kind/session, never reused |
| 40 | 8 | First-sample capture PTS, ns since session capture epoch |
| 48 | 4 | Duration ns; rational frame duration rounded once |
| 52 | 4 | Complete unit byte length |
| 56 | 2 | Fragment index, zero based |
| 58 | 2 | Fragment count |
| 60 | 4 | Fragment byte offset within unit |

Wi-Fi payload follows the header; total datagram length determines fragment length. Reassembly requires exactly covering `[0, unit_length)` with consistent metadata and no overlapping conflicting bytes. Exact duplicates are ignored. Cap video unit size at **8 MiB**, audio at **64 KiB**, fragment count at **16384**, outstanding video units at **4**, and all reassembly allocation at **32 MiB/session** (reserve against declared lengths before allocation). TTL and useful-deadline limits normally expire much sooner. Config limits may be smaller; parser caps are safety limits, not permitted latency/backlog budgets.

USB media record: 4-byte big-endian record length then 64-byte header then whole unit; fragment index=0/count=1/offset=0. Limit record size to header plus unit maximum; handle partial reads and EOF mid-record. TLS/QUIC provide integrity/confidentiality, so there is no homegrown media encryption or CRC security layer.

QUIC media binding uses a separate non-media datagram with magic `TCMB`, version byte 1, session ID 16 bytes, epoch u32 and random **32-byte single-use media token** (57 bytes total). Token is sent only inside the already server-authenticated QUIC connection and obtained over authenticated control. Repeat at 100 ms up to 1 second until `MediaBound` arrives over control; server idempotently acknowledges on the same QUIC connection, but rejects reuse on a different connection. Do not allocate reassembly/decode resources before binding. Token expires after 5 seconds. Feedback/NACK initially uses control, not undocumented QUIC streams.

### 7.2 Pairing and identities

Threat model: hostile/untrusted LAN clients and discovery spoofing, wrong USB peer, stolen session tokens, malformed media, local log exposure. Root/physical compromise of the endpoints is outside the protection offered by this application.

Generate per-install endpoint identities. Linux: TLS certificate/key through maintained certificate tooling (e.g. rcgen) plus application signing key; store in user-only XDG state directory. iOS: application signing key in Keychain; USB TLS server identity uses a self-issued certificate and matching private key accessible as a `SecIdentity`. A small M0 prototype must prove generation/import/Keychain lookup with the selected iOS SDK; use maintained X.509 tooling (e.g. Apple's swift-certificates) and supported Security APIs, not handcrafted ASN.1/crypto. Validate Keychain persistence and revocation behavior. [Apple Swift X.509 library](https://github.com/apple/swift-certificates)

First Wi-Fi pair: receiver shows QR containing receiver address/ports, exact TLS leaf-certificate SHA-256 fingerprint, protocol version and a random 256-bit one-time token valid 120 seconds. Phone scans, validates the pinned certificate during TLS/QUIC handshakes, sends pair request with its application public key/token; receiver consumes token and records peer on explicit user confirmation. Both display matching identity/fingerprint summary.

USB-only first pair: phone shows its USB server certificate fingerprint and one-time token as QR/copyable text; receiver operator imports it through the local CLI/UI and confirms phone identity. Phone confirms receiver's presented application public key locally before persisting. Do not bootstrap trust from an unauthenticated LAN announcement or silently from cable presence. Support manual fingerprint/token entry if the PC has no camera.

For returning peers, use a fresh 32-byte challenge and signature over a fixed, versioned binary transcript containing role, both stored identity IDs, nonce and new session ID. Application identity suite v1 is Ed25519: CryptoKit `Curve25519.Signing` on iOS and a maintained Rust implementation, 32-byte public keys and 64-byte signatures. Define identity ID as SHA-256 of the raw public key. Transcript bytes are ASCII `TitanCam-auth-v1`, one role byte (1 phone, 2 receiver), phone identity ID (32 bytes), receiver identity ID (32 bytes), challenge nonce (32 bytes), and session ID (16 bytes), in that order. Role-distinct proofs prevent reflection; each side verifies the remote peer's proof. This signing identity is separate from a TLS certificate's key type. Auth proofs are verified before commands/media; reject repeated/expired nonces. USB channels additionally carry short-lived control-issued binding tokens and connection roles. Wi-Fi control and media must pin the same expected receiver certificate. Certificate renewal requires authenticated rotation or explicit re-pair; never disable verification because a certificate changed.

Certificate pin verification still verifies possession via the TLS handshake and validates expected identity/certificate constraints; no permissive fallback. QUIC uses TLS 1.3 internally. Disable 0-RTT for auth, configuration and media in v1. Rate-limit handshake/pair attempts (5/minute/peer proposed); cap concurrent unauthenticated connections (4); authentication timeout 5 seconds. Discovery carries no secrets. Provide revoke/unpair on both ends; delete peer credentials and end live sessions.

Bind control listeners only on selected LAN interfaces unless user explicitly enables broader access. No UPnP or Internet port exposure. Local receiver control is a Unix-domain socket with restrictive permissions; metrics default localhost. Linux private files 0600 and containing directories 0700; iOS keys use an appropriate device-only Keychain accessibility class. Logs redact secrets/device IDs. Session media is not recorded by default.

## 8. Linux receiver, NVDEC, and outputs

### 8.1 Runtime architecture

Tokio tasks handle discovery, encrypted transports, control, validation/reassembly, metrics and session management. GStreamer runs with its own main-context/bus handling and a bounded ingress bridge. PipeWire uses its own real-time loop. No media API is assumed safe on arbitrary executor threads. Avoid locks spanning a callback or `.await`.

Provide CLI first and an optional local UI later. `doctor` checks distro/kernel, GStreamer/plugin versions, NVIDIA GPU/driver, decoder factories and actual test decode, PipeWire/WirePlumber, module availability, `/dev/video*` permissions, usbmuxd socket/device access, and bindable ports. Missing accelerated decode must be visible. Do not assume Ubuntu's `plugins-bad` build includes a working NVDEC path. [gstreamer-rs bindings](https://docs.rs/gstreamer/latest/gstreamer/), [PipeWire Rust bindings](https://docs.rs/pipewire/latest/pipewire/)

### 8.2 Video pipeline

Conceptual H.264 path (build programmatically, negotiate actual caps):

```text
appsrc (is-live=true, format=time, do-timestamp=false, bounded nonblocking)
  caps video/x-h264, stream-format=byte-stream, alignment=au
  → h264parse → nvh264dec → bounded raw-output dispatch
                                      ├ GPU preview
                                      └ GPU/CPU convert-scale → system-memory V4L2 writer
```

HEVC replaces parser/decoder with `h265parse`/`nvh265dec`. CPU fallback uses available software decoders, with a profile downgrade if needed. Set PTS/duration from mapped capture time. Start with at most two compressed AUs at appsrc plus a byte/time cap. Keep compressed queues non-leaky and perform coordinated upstream reference recovery on overrun; arbitrary compressed leaky drops are unsafe. Raw frames after decoding may be dropped latest-first safely to stay fresh. [appsrc](https://gstreamer.freedesktop.org/documentation/app/appsrc.html)

The nvcodec plugin offers H.264/HEVC decode and CUDA conversion/download elements. NVIDIA's matrix lists RTX 4050 Laptop as Ada with NVDEC support including H.264 and HEVC. Test dimensions/profile/bit-depth/caps on the actual system; matrix capability is not a benchmark and AV1 decode support does not make the iPhone an AV1 encoder. [nvcodec](https://gstreamer.freedesktop.org/documentation/nvcodec/), [H.264 decoder](https://gstreamer.freedesktop.org/documentation/nvcodec/nvh264dec.html), [HEVC decoder](https://gstreamer.freedesktop.org/documentation/nvcodec/nvh265dec.html), [NVIDIA support matrix](https://developer.nvidia.com/video-encode-decode-support-matrix)

Probe optional decoder delay properties rather than applying Jetson-specific `nvv4l2decoder` controls to desktop NVDEC. Keep preview frames in CUDA/GL memory where the actual negotiated interop supports it. System-memory V4L2 output generally requires a download/copy; there is no blanket end-to-end zero-copy guarantee. On hybrid laptops, display GPU/PRIME behavior may add transfer cost. Check plugin ABI/driver dependencies and CUDA runtime compilation requirements before deployment.

A `tee` alone does not isolate consumers: each branch needs independent bounded queues/dispatch. Schedule preview via a common presentation clock; sync-disabled sinks are only for a separately labelled minimum-delay diagnostic mode. Stable webcam output can use a dedicated latest-frame writer so stalls do not block ingress. Record decode complete, preview submit and webcam write separately; a write completion is not downstream display time. [GStreamer queue](https://gstreamer.freedesktop.org/documentation/coreelements/queue.html), [tee](https://gstreamer.freedesktop.org/documentation/coreelements/tee.html)

### 8.3 Virtual webcam

Install v4l2loopback/DKMS matching the host kernel, with headers/compiler and signing/enrollment if Secure Boot requires it. Provide instructions and an explicit setup command; normal receiver service is unprivileged. Choose an unused video node, e.g. `/dev/video42`, with `exclusive_caps=1`. Module controls can fix negotiated format, duplicate frames for nominal cadence and show a timeout image; negotiate supported caps rather than assuming NV12/YUYV works everywhere. [v4l2loopback upstream](https://github.com/v4l2loopback/v4l2loopback/blob/main/README.md)

Default compatible consumer output: 1920×1080 SDR at 30 FPS when the consumer cannot accept 60; source may remain 1080p60/4K. Support 720p and 1080p output, and qualify 4K separately. Prefer a supported YUV format; fallback YUYV with explicit conversion. At 1080p60 YUYV the copy volume is about 249 MB/s, at 4K60 about 995 MB/s, so scale before download when supported. Consumer compatibility and memory bandwidth can dominate an otherwise fast decode path.

Keep output caps stable across source-profile downgrades: scale current source into the established output format. Changing consumer caps is explicit and may require restarting its producer/consumer. On outage show a slate after 500 ms and keep device cadence; do not freeze the last image indefinitely. Frame duplication is separately counted, never called received FPS.

### 8.4 Virtual mic and common scheduler

Publish native PipeWire output stream with `media.class = Audio/Source`, stable node name `titancam_mic`, readable description, and negotiated 48 kHz mono/stereo PCM. Use PipeWire's supported stream/SPA format contracts and actual graph timing. Ordinary playback into a sink is not a microphone. WirePlumber policy and client visibility must be verified. [PipeWire stream model](https://docs.pipewire.org/page_streams.html)

Feed timestamped audio into a preallocated SPSC ring; PipeWire process callback consumes the requested number of samples. On underrun perform bounded PLC/silence and increment counters. Respect actual graph quantum/rate; request low latency without forcing global settings. Example quantum 128/256 at 48 kHz is ~2.67/5.33 ms, but other clients/devices can alter effective latency. [PipeWire configuration](https://docs.pipewire.org/page_man_pipewire_conf_5.html)

Decode Opus and resample on a worker. USB PCM skips lossy encode/decode but still needs clock correction. Optional speaker monitor is off by default to avoid echo. Native PipeWire video-source export is a later output adapter; V4L2 remains the v1 broad compatibility route. Separate webcam/mic consumer APIs do not guarantee downstream A/V sync; measure OBS/browser behavior and provide a user A/V offset.

## 9. Clock synchronization and A/V presentation

The sender timeline is capture PTS, not encoder-completion or packet-arrival time. Receiver estimates `R ≈ a × S + b`, where S is sender host time, R receiver monotonic time, a captures drift, and b offset. Store the relation between normalized capture PTS and sender host time in Start/config metadata.

Clock exchange: receiver records R1, sender receives at S2 and sends at S3, receiver receives at R4. Offset sender-minus-receiver is approximately `((S2−R1)+(S3−R4))/2`; network round trip approximately `(R4−R1)−(S3−S2)`. Retain low-RTT samples, reject outliers, fit slow drift and report uncertainty/asymmetry. Eight startup samples, then one per second, is an initial policy. These clocks are unrelated numeric epochs; use overflow-safe signed arithmetic before normalization.

Choose receiver audio graph clock as steady playback master and map sender time into it; video uses the same target playout timeline. Audio adaptive resampling follows long-term ring fill/drift (initial clamp ±300 ppm), not per-packet jitter. A persistent offset beyond 20 ms requires a controlled discontinuity reset/fade, not ever-increasing buffering or audible fast rate changes. Video is dropped/held within one frame interval to follow the timeline.

Calibrate codec lookahead, actual audio input latency and output graph latency. Preserve first-sample timestamps through Opus framing/PLC/resample. PTS starts at epoch origin; receiver subtracts pipeline base time when generating GStreamer running-time timestamps and avoids negative startup PTS through a negotiated start barrier. Clock and codec resets generate explicit discontinuity metadata.

Healthy-link objective: internal A/V skew p95 ≤20 ms, maximum ≤40 ms over a 60-minute run. Report skew with positive value defined as audio presented later than matching video. Preview-only fastest mode may bypass sync for diagnosis and must be labelled accordingly. An external flash/click test validates actual skew; clock estimates alone cannot prove it.

## 10. Operating profiles and thermal policy

All rates below are starting values for hardware SDR encoding, not guaranteed camera-native quality. Bitrates are video payload Mbit/s, not total network usage. Both codec ranges require visual/latency validation. H.264 is the baseline default in all profiles; HEVC becomes a per-device preferred choice only after qualification demonstrates benefit.

| Parameter | Best efficiency / saver | Balanced | Maximum quality / performance |
|---|---|---|---|
| Capture default | 1280×720, 30 FPS | 1920×1080, 60 FPS; optional 30 for stationary scenes | 3840×2160, 60 FPS **conditional qualification**; 4K30/1080p60 fallback |
| H.264 payload target/range | 4 / 2–6 | 14 / 8–24 | 65 / 40–100 |
| HEVC alternative target/range | 3 / 1.5–5 | 10 / 6–18 | 45 / 25–80 |
| USB audio | PCM mono 48 kHz, 5 ms | PCM mono or stereo, 5 ms | PCM stereo when supported, 5 ms |
| Wi-Fi audio | Opus mono 64 kbit/s, 20 ms | Opus mono 96 or stereo 160 kbit/s, 10 ms | Opus stereo 192–256 kbit/s, 5 ms restricted low-delay |
| Wi-Fi jitter start/clamp | 15 / 10–40 ms | 10 / 5–30 ms | 5 / 0–20 ms on proven clean link |
| USB playout start | 10 ms | 5 ms | 3–5 ms |
| Preview/UI | Preview off/dim option; stats 1 Hz | Preview 30/60; stats 1 Hz | Full preview optional; stats 1 Hz |
| Effects/HDR | Off | Off | Opt-in only after measurement |
| Saver fallback ladder | 720p30 → 540p30 → 480p24 | 1080p60 → 1080p30 → 720p30 | 4K60 → 4K30 → 1080p60 → 1080p30 → 720p30 |

The maximum-quality choice is not automatically the minimum-latency or maximum-sustained-FPS choice. Expose a maximum-profile preference: preserve spatial detail (4K30 next) or preserve motion (1080p60 next); choose the documented detail ladder by default. The maximum bitrate is constrained by measured path capacity and IDR bursts. Enable 4K60 only after a 60-minute physical-device qualification with acceptable thermals, effective FPS and deadlines.

Observe `ProcessInfo.thermalState`, battery/charging/Low Power Mode where available, and camera `systemPressureState`. iOS public APIs do not provide a dependable numeric SoC temperature sensor. Serious pressure/thermal state reduces FPS and optional work immediately; critical/shutdown stops capture cleanly, signals the receiver and waits for recovery. Return upward only after 30 seconds of stable nominal/fair state and one level at a time. Local thermal safety overrides a remote maximum request. [Apple camera device/pressure API](https://developer.apple.com/documentation/avfoundation/avcapturedevice)

Energy optimization: no simultaneous duplicate encoders by default, no CPU RGB conversion, coalesce telemetry, avoid busy polls, disable unnecessary preview/effects, use bounded pools, stop inactive sessions, and do not spin-retry disconnected devices. USB power/charging may increase phone heat; measure externally or separate charging/no-charging trials. Laptop GPU wake-up may cost more power than efficient CPU decode at 720p; saver mode chooses the measured lower-total-energy path rather than assuming NVDEC always wins.

## 11. Latency budgets and measurable acceptance

Definitions: **glass-to-glass** physical scene change to Linux display photons; **capture-to-output** capture PTS to preview submission/webcam write/audio graph presentation; **transport** sender submit to receiver arrival; **downstream** receiving application buffering/display. Never collapse them into one number.

Proposed healthy-link glass-to-glass targets after warm-up (p50 / p95, milliseconds):

| Profile | USB preview | Wi-Fi preview | Qualification note |
|---|---|---|---|
| Saver 720p30 | ≤80 / ≤120 | ≤100 / ≤150 | Longer frame interval; evaluate energy saved |
| Balanced 1080p60 | ≤60 / ≤90 | ≤80 / ≤120 | Primary v1 acceptance target |
| Maximum 4K60 | ≤75 / ≤110 | ≤95 / ≤150 | Conditional target; workload/thermal/bandwidth may require downgrade |

Capture-to-virtual-mic starting objectives: USB p95 ≤60 ms, Wi-Fi p95 ≤90 ms balanced. Virtual webcam downstream programs can add tens/hundreds of milliseconds; measure and report them separately, with an initial engineering objective of ≤1 extra frame in TitanCam's raw output adapter under a consumer that keeps up. These values are product targets, not published vendor benchmarks or contractual guarantees.

Balanced USB 60 FPS diagnostic stage budgets: sensor/exposure/readout/availability 8–25 ms; encode 2–10; send/transfer 1–8; intentional buffer 0–10; decode/convert 2–10; display queue/scanout 8–25. Their ranges overlap in pipelined operation and do not prove an achieved sum. Wi-Fi adds path variability and usually 5–30 ms bounded jitter allowance. Low-light exposure, stabilization, display compositor, software decode and output copies can dominate.

Healthy qualification conditions: 60-minute run, 5-minute warm-up; built-in mic; adequate lighting; fixed lens/exposure policy; clean LAN p95 RTT ≤10 ms and loss <0.1%; adequate measured goodput (payload plus overhead and burst reserve); PC power mode and monitor refresh recorded. Objective effective input FPS ≥99% of requested, expired/dropped video ≤0.5%, and no freeze >250 ms in that controlled run. Report duplicates separately and record p99 even when the acceptance table uses p95.

Impaired-link behavior is evaluated for bounded delay/recovery, not unchanged quality/no frame loss: apply 1/3/5% random loss, 50/100 ms burst outages, RTT 20/50/100 ms, ±10/30 ms jitter, 100/60/30/10 Mbit/s bottlenecks, and competing traffic. Use both synthetic `tc netem`/traffic shaping with documented direction/topology and a real congested AP. Once capacity falls below the negotiated profile, downgrade visibly. Reliable archival delivery belongs to a separate optional recording path.

Measure scene/display with an external ≥240 FPS camera (quantization ~4.17 ms), repeated flash transitions, randomized phase and enough samples (≥1000 preferred) for percentile estimates. Report instrument limits and clock uncertainty. Store machine-readable anonymized benchmark output and plots, plus capture/encoder/transport/decoder/scheduler stage traces from sampled instrumentation. Actual display latency needs external evidence.

## 12. Recovery state machine and isolation

```text
Idle → Discovering → Connecting → Authenticating → Negotiating → Priming → Streaming
                                                                      ↕
                                                                    Degraded
any active state → Interrupted / Reconnecting → Authenticating → Negotiating → Priming
any state → Stopping → Idle
```

Every transition has reason, timeout, cancellation and owner. Transport epoch changes on reconnect/switch; configuration ID changes on source/codec/format changes. New session ID after receiver/app restart or capture clock reset. USB channel reopen within a live control session keeps the capture timeline but uses a new transport epoch. Do not compare two unrelated capture epochs.

Heartbeat interval 500 ms, degraded at 1 second without progress, reconnect after 2 seconds without valid control response. Media-silent watchdog also detects a live control channel with frozen video. Retry with jittered 100/250/500/1000/2000 ms backoff; cap rate and release old handles. Gate permissions/auth failures on user action instead of busy retry.

Reconnection steps: stop enqueuing old transport work; invalidate tokens/epoch; connect/auth; exchange effective capabilities/config; settle clock if needed; reset decoder/reassembly; force and validate IDR; prime bounded audio; start aligned output. Post-connect target from authenticated link ready to live picture/audio ≤1 second; complete warm USB replug target ≤3 seconds, Wi-Fi recovery ≤5 seconds after reachability returns. These exclude required unlock/trust/permissions and failed AP/device service recovery.

Loss of video does not stop mic unless session policy requires it. Missing audio uses short PLC then silence; after 500 ms show an audio outage indicator. Missing video shows the slate after 500 ms. GStreamer errors rebuild only affected pipelines; repeated decoder failure falls back to software and renegotiates performance. PipeWire disconnect rebuilds the source while network capture stays bounded.

USB preference is a user setting. On cable failure and paired reachable Wi-Fi, create/authenticate a candidate transport, configure a new epoch and start at a fresh IDR/audio boundary. If make-before-break is possible, use one encoder and bounded duplicated media briefly (≤250 ms) rather than two encoders; receiver accepts one selected epoch at a time. Clock continuity and stream/caps stability are more important than claiming seamless failover. Report switch gap and bandwidth cost.

Camera interruption/background/lock/thermal shutdown is visible. On foreground recovery rebuild the session/encoder as necessary, renegotiate changed audio route/formats and send new epoch/config. Do not advertise guaranteed screen-locked capture on iPhone 13. [Apple background interruption reason](https://developer.apple.com/documentation/avfoundation/avcapturesession/interruptionreason/videodevicenotavailableinbackground)

## 13. Telemetry, diagnostics and operations

Every component exposes counters, gauges and latency histograms. Counters: capture drop reasons, encoded AUs/bytes, send drops, repairs, received/complete/expired/duplicate fragments, reference resets, decoder errors, raw output drops/duplicates, audio PLC/underruns/overruns, reconnects and profile transitions. Gauges: effective dimensions/FPS/codec/bitrate, queue bytes/oldest age, RTT/jitter/loss, clock drift/uncertainty, A/V skew, CPU/RSS, GPU decode activity, thermal/pressure state and actual audio route.

Stats UI updates at 1 Hz; network feedback 10 Hz; stage tracing sampled rather than logging every video fragment. Export local Prometheus/OpenTelemetry-compatible metrics if desired, without high-cardinality session IDs/UDIDs in labels. Structured logs with reason codes, rotation and redaction. No remote analytics service is required. Diagnostic bundle contains effective configuration, versions, anonymized counters and recent errors; media/keys/tokens are excluded unless explicitly chosen.

Config files use XDG paths and named profiles. User service launches after PipeWire is ready, and exposes a restrictive local control socket. Startup leaves streaming off until a paired phone starts/accepts a session. Provide clean uninstall without breaking the host's unrelated audio/camera configuration. Setup instructions cover udev/group permissions, trust prompts, DKMS/kernel updates and module signing; never require a root-running receiver.

## 14. GitHub Actions, signing and local sideloading

### 14.1 Runner strategy

Use GitHub Actions orchestration with Linux x86_64 jobs for the receiver and **macOS/Xcode jobs for iOS**. A Linux server cannot run a supported native Xcode/iOS SDK build. If the user's servers are Linux-only, use a GitHub-hosted macOS runner or add a maintained Apple macOS self-hosted runner. Check current runner image inventory and installed Xcode before choosing a pinned label/version; avoid relying on a floating `macos-latest` SDK. [GitHub runner images](https://github.com/actions/runner-images)

Commit an Xcode project and shared scheme. Pin Rust toolchain, Swift packages, Opus source, package scripts and action full commit SHAs. Record actual Xcode/SDK, OS image, Rust, GStreamer, PipeWire and native-library versions in artifact metadata. Build device arm64 and appropriate simulator architectures separately. Simulator builds validate logic/compilation, not real encoding/camera/USB performance.

### 14.2 Workflow matrix and outputs

| Workflow/job | Trigger and runner | Work and artifacts |
|---|---|---|
| protocol + Rust checks | PR/push, isolated Linux | fmt/clippy/tests/golden vectors; fuzz smoke; synthetic decoder/audio logic tests |
| iOS checks | PR/push, macOS pinned Xcode | XCTest simulator + unsigned arm64 device compile; `.xcresult`, compiler metadata |
| Linux package | trusted branch/tag, Ubuntu baseline | `cargo build --release --locked`, Debian package and tarball, dependency manifest, symbols separate |
| iOS unsigned-for-resigning | trusted branch/tag, macOS | unsigned `.app` and optional explicitly labelled `Payload/TitanCam.app` zip/IPA; not directly installable |
| iOS signed export | protected tag/manual release, macOS | certificate/profile validation, archive/export `.ipa`, entitlements and expiry summary, hashes |
| hardware qualification | trusted dispatch, isolated device/GPU runners | NVDEC, PipeWire/V4L2, actual iPhone/USB/Wi-Fi benchmarks and reports |

PR jobs have no signing secrets and do not execute untrusted contributions on persistent private/hardware runners. Separate privileged release jobs, protected environments and minimal token permissions. Pin actions by full SHA and review updates. Never build PR head code with secrets under `pull_request_target`. [GitHub Actions secure-use guidance](https://docs.github.com/en/actions/reference/security/secure-use)

Linux dependencies include GStreamer dev/runtime plugins, PipeWire dev/runtime, libusbmuxd dev/runtime, pkg-config, compiler, and optional CPU decoders. Pin package snapshots/container digest for release reproduction, while retaining update policy. Build on the oldest supported ABI baseline; normal packages depend on host libraries. Kernel module and NVIDIA driver are installed for the host, not built into an AppImage. ARM64 can be a later separately tested artifact; do not claim it passed NVIDIA/USB testing because it cross-compiled.

Provide repository scripts wrapping these commands, with configurable paths and deterministic build folders:

```sh
# Simulator/device syntax templates; selected SDK/Xcode must be validated.
xcodebuild -project ios/TitanCam.xcodeproj -scheme TitanCam \
  -configuration Debug -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath build/ios-simulator CODE_SIGNING_ALLOWED=NO build
xcodebuild -project ios/TitanCam.xcodeproj -scheme TitanCam \
  -configuration Release -destination 'generic/platform=iOS' \
  -derivedDataPath build/ios-device CODE_SIGNING_ALLOWED=NO build

# Signed route: signing configuration comes from protected CI inputs.
xcodebuild -project ios/TitanCam.xcodeproj -scheme TitanCam \
  -configuration Release -destination 'generic/platform=iOS' \
  -archivePath build/TitanCam.xcarchive archive
xcodebuild -exportArchive -archivePath build/TitanCam.xcarchive \
  -exportPath build/export -exportOptionsPlist packaging/ExportOptions.plist
```

Do not commit an invented Xcode export-method value. Validate `xcodebuild -help` on the pinned version and generate the matching development/ad-hoc export options, signing style, team ID and provisioning-profile mapping. Successful unsigned compilation is separate from successful signed archive/export and separate again from device installation.

### 14.3 Signing prerequisites and secret handling

For normal development/ad-hoc installation, the bundle ID, Apple team, certificate/private key, entitlements, provisioning profile and device list must agree. A `.p12` without its private key is insufficient. An App Store Connect API key alone is not the app's code-signing identity. Ad-hoc provisioning requires registered devices and an appropriate distribution certificate; paid-team workflows are the reliable CI route. Free/personal-team and tool-managed signing have different restrictions and renewal behavior, to be verified against the actual account/tool. [Apple ad-hoc profile requirements](https://developer.apple.com/help/account/provisioning-profiles/create-an-ad-hoc-provisioning-profile/), [registered-device distribution](https://developer.apple.com/documentation/xcode/distributing-your-app-to-registered-devices)

Protected signing inputs: certificate P12 base64, its password, profile base64, team ID, expected bundle ID and identity/profile identifiers. Create an ephemeral keychain with a random job password; import identity and allow only necessary codesign access; install profile in the location used by the selected Xcode; verify profile type, expiry, application identifier/entitlements and device coverage; archive/export; verify code signature and final embedded profile; clean up keychain/profile/temp secrets with an `always()` step. Cleanup is especially important on persistent self-hosted Macs. Base64 is encoding, not secret protection. [GitHub Apple certificate workflow](https://docs.github.com/en/actions/how-tos/deploy/deploy-to-third-party-platforms/sign-xcode-applications)

Do not include Apple signing material in artifacts or logs. Exported installation artifacts necessarily contain an embedded provisioning profile when that route uses one; keep device-bearing development/ad-hoc IPAs access-controlled. Publish public Linux artifacts independently. Release manifests include commit, protocol version, build versions, hashes, SBOM/license notices, signing status and qualification status. No automatic App Store/TestFlight upload is required.

### 14.4 Local installation handoff

Final sideloading uses **the user's local iOS sideloading tool**. Earlier context names iLoader/iLock, but exact tool/version/support is not established. Implementation must inspect the installed tool's help/version and primary documentation before providing commands. No cloud runner needs physical access to the phone for artifact compilation/export.

Two valid handoff routes: (A) CI exports a valid development/ad-hoc signed IPA for the registered phone and the local tool installs it; (B) CI emits a clearly labelled unsigned device app/IPA container and a compatible local tool re-signs/provisions it, then installs. Route B depends on actual tool capability; an unsigned IPA is not inherently installable. If neither signing route is available, compilation can finish but installation remains pending credentials/tool support.

Local checklist: verify artifact hash; verify signing status and entitlements; phone unlock/trust/pairing as required; Developer Mode when required; install using the actual documented tool; launch; approve camera/microphone/local-network access; pair TitanCam; run `doctor`; qualify USB first then Wi-Fi. Record actual certificate/profile expiry and required refresh route. A tool cannot bypass Apple's valid-signature/provisioning requirements by merely copying the IPA.

## 15. Implementation acceptance and unresolved gates

Follow milestones and mandatory tests in AGENTS.md. Before release there must be physical evidence for: app TCP listener access through usbmuxd; iOS certificate identity and authenticated channels; QUIC interoperability and useful DATAGRAM limits; hardware encode/no-reorder/IDR behavior; NVIDIA decoder capabilities and power; webcam/mic consumer behavior; clock/sync accuracy; profile thermals and recovery; and the actual local sideload workflow.

Unknown deployment inputs are the phone's current iOS version, Ubuntu/kernel/driver versions, exact RTX SKU/display topology, AP configuration, Apple account/signing assets, local sideloading-tool identity and whether the user's server fleet includes macOS. These do not prevent coding the baseline, but they prevent claiming those hardware/install targets have passed. Capture them during M0 and in the qualification report.

Useful later enhancements: separately paced local recording of the original compressed stream; user exposure/focus presets; phone mounting/orientation controls; measured temporal layers/FEC; qualified HDR-to-SDR tone mapping; QR pairing UI; anonymized diagnostic export; and deliberate GPU denoise/sharpening presets. Each feature must preserve queue bounds, credential protection, source metadata and transparent effective-profile reporting.


## 16. Implemented development contract

The executable preview implements a subset of this target architecture. Read
[ADR 0001](docs/adr/0001-development-wire-contract.md) for the exact handshake,
MediaReady/StreamingReady/ConfigureApplied barriers, iOS 17.4 deployment target,
implemented bounds and remaining qualification gates. Source and passing compilation
do not establish physical-device performance. See docs/NEXT_STEPS.md for evidence.
