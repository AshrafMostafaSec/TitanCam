# TitanCam improvement plan — existing project

Date: 2026-10-01 (Africa/Cairo). Audited source: `32a5008` (0.2.3). This is an implementation plan, not a claim that the features below have shipped. Read with the root AGENTS.md and ARCHITECTURE.md.

الخطة مبنية على المشروع الموجود فعلًا على الديسكتوب. الأولوية: تشخيص التقطيع وإصلاحه، ثم التحكم الكامل من لينكس في الكاميرا والصوت والصورة، ثم تأهيل 4K60 وتحسين كفاءة Wi-Fi. كل مرحلة لها اختبار قبول؛ وجود GPU أو نجاح البناء وحده لا يثبت السلاسة.

## Implementation decisions for 0.3.0

The owner authorized implementation and builds on 2026-10-01; physical two-hour qualification is deferred by the owner. Deliver camera/lens and public AVAudioSession microphone data-source selection from Linux, acknowledged configuration commands, receiver gain/mute/mirror/flip, separate audio/video scheduling, bounded reordering/repair, honest decoder/FPS telemetry, and matched build artifacts. Preserve plaintext LAN scope. Existing HEVC selection becomes available in the GUI without claiming qualified 4K60 or enabling HDR. Long-session qualification remains an explicit release limitation.

Microphone selection uses availableInputs, setPreferredInput and setPreferredDataSource, with effective route verification; sources that the OS does not expose are unavailable. See [Apple microphone selection](https://developer.apple.com/library/archive/qa/qa1799/). All state-changing configuration commands have request IDs and bounded acknowledgements; configuration failure returns a recoverable error and restores the last working capture configuration.

## 1. Scope and verified baseline

Keep the current Swift/AVFoundation/VideoToolbox sender and Rust/GStreamer/PipeWire receiver. Evolve the project incrementally; do not replace it with a new prototype. Preserve the established trusted-LAN product choice: plaintext TCP control/USB and UDP media, automatic discovery, no phone preview or pairing UI. A transport change involving TLS needs a separate product decision; QUIC cannot preserve plaintext.

| Area | Current evidence | Required work |
|---|---|---|
| Desktop branding | Generic `camera-video` icon; no branded GTK window icon | Existing iOS artwork reused for Linux icon, launcher and future package; this small fix is applied locally |
| GPU | Local diagnostic decoded 30 generated H.264 frames with `nvh264dec`; RTX 4050 Laptop, driver 595.91.07 | Verify the active phone session's decoder, decoded/displayed FPS and negotiated memory paths, including HEVC |
| Audio | PipeWire source publication succeeded; diagnostic showed 2 underruns | Qualify sustained real microphone playback; add gain/mute/meters |
| Cameras | CaptureEngine selects/caches rear wide camera | Discover front/rear lenses and their actual compatible formats; transactional switch |
| Linux controls | Profile, preview, webcam; Apply restarts receiver | Live control API with acknowledgements, gain, flip, source and format selection |
| Smoothness | Sender pacing/adaptation fixes exist in 0.2.3 | Instrument before assigning root cause; shared audio/video worker and fixed deadlines need experiments |
| Wi-Fi | Paced 1100-byte datagrams, 60 ms assembly expiry, IDR recovery | Delay/loss-aware adaptation, bounded repair, optional measured FEC |
| Max profile | Actual code requests 4K60 **H.264 65 Mbit/s**, not HEVC | Qualify HEVC efficiency, then select codec from measured results |
| Lifecycle | Phone intentionally stops on background; interruptions mostly become generic failures | Typed interruption reasons, safe resume, screen-dimming foreground mode |

These diagnostics are synthetic, short, and do not qualify phone capture or 4K60. USB enumeration was unavailable during this audit. No physical-phone stream was reproduced in this turn. GPU utilization from `nvidia-smi` is machine-wide and cannot identify one application's decoder usage.

## 2. Files and architectural boundaries

| Responsibility | Existing files | Planned extraction/addition |
|---|---|---|
| Capture, capability discovery | `ios/TitanCam/Capture/CaptureEngine.swift` | `CameraCatalog.swift`, `CaptureConfiguration.swift`, `CaptureLifecycle.swift` |
| Encode/correct color | `Capture/VideoEncoder.swift`, `VideoHDRPolicy.swift` | Preserve hardware evidence and SDR setter order; capability-gated encoder policy |
| Session/thermal/adaptation | `Streaming/StreamCoordinator.swift` | `NetworkAdaptation.swift`, `ThermalPolicy.swift`, typed state machine |
| Pacing/repair | `Streaming/DatagramSender.swift`, `MediaSendQueue.swift` | Bounded sent-packet cache, wire-budget telemetry |
| Cross-language contracts | `crates/titan-protocol/src/lib.rs`, `Models/Protocol.swift`, `protocol/` | Versioned capability, command, acknowledgement and feedback fixtures |
| Linux orchestration | `crates/titan-receiver/src/main.rs`, `local.rs`, `session.rs` | `control_api.rs`, `clock.rs`, `adaptation.rs`, session supervisor |
| Video/audio output | `crates/titan-media/src/lib.rs`, `mic.c` | `video.rs`, `audio.rs`, `scheduler.rs`, `audio_gain.rs`, transform policy |
| Desktop | `scripts/desktop-launcher.py`, `packaging/` | Small UI/controller modules, structured local control client, brand assets |
| Evidence | `crates/titan-bench/`, `ios/TitanCamTests/`, `docs/` | Regression fixtures, impairment runner, qualification reports |

Module extraction follows behavior tests and profiling. Keep ownership explicit: one capture owner, one session supervisor, one decoder, independent output branches, one audio producer and real-time consumer. Remove duplicate/obsolete code only after tracing callers and compatibility; preserve older research as historical evidence. No unrelated repository cleanup.

## 3. P0 — diagnose and eliminate stalls

1. Add timestamped stage counters: captured, encoder-submitted/completed/dropped, sender age/bytes, received fragments, complete/expired units, decoder-submitted/decoded, preview-presented estimates, webcam-delivered and consumer-observed FPS. Accepted encoded units are not displayed frames.
2. Record per-stage queue age p50/p95/p99, loss/reordering, IDR requests/rate/bytes, decoder resets, GStreamer QoS/lateness, PipeWire underruns and A/V skew. Export actual requested/effective dimensions, FPS, codec and bitrate, session ID, transport and fallback reason. No simulated values.
3. Run the same source over USB and Wi-Fi at 720p30, 1080p60 and 4K60: decode-only, preview-only, webcam-only, both outputs, and audio disabled/enabled. Compare NVDEC with an explicit CPU test at feasible settings. Record lighting/exposure, display refresh, host load and laptop power mode.
4. In `session.rs`, audio sleeps until playout inside the shared worker. Measure that wait's effect on video dequeue age; then give audio its own bounded scheduler/ring producer. Maintain one shared presentation clock. Prioritize audio at ingress without letting audio playout sleep stall video.
5. The current reassembler emits completed units without an explicit sequence reorder window; out-of-order completion can look like reference loss to the media worker. Add a bounded deadline-aware completed-AU reorder stage. Never add a multi-second buffer or continue dependent pictures after missing a reference.
6. Probe caps/memory features at decoder and output pads. NVDEC is the hardware codec engine; ordinary UDP reception still uses OS/CPU. `num-output-surfaces=0` is auto selection, not proof of zero-copy. Preview plus V4L2 may force downloads: measure. At 4K60 NV12 raw payload is approximately 746 MB/s before copy overhead, even when the network stream is only tens of Mbit/s.
7. Keep one decode and independently bounded raw branches. Do not blindly enable leaky compressed queues. If appsrc refuses input or the reference chain is damaged, clear obsolete compressed data, rate-limit IDR recovery and resume on verified independent data.
8. Runtime GPU loss must emit a typed fallback and requalify feasible settings. Do not silently retain a 4K60 label when CPU cannot keep up. No GPU clock/power changes affecting other applications.

**Exit gate:** report the measured cause(s), before/after runs at identical settings, no unexplained repeated decoder reset, stable A/V sync, and no growing backlog. A short synthetic diagnostic is insufficient.

## 4. P1 — desktop control plane and professional GUI

Add an unprivileged Unix-domain control socket under the user's runtime directory, with private directory/socket permissions, bounded framed messages, protocol version, request ID and response deadline. Do not expose the GUI control API over a new public TCP port. Split process supervision from widget rendering. Read health snapshots atomically; the current stats-file writer should use temporary-write plus rename to avoid partial JSON reads.

Commands: `GetCapabilities`, `GetStatus`, `SetCamera`, `SetCaptureFormat`, `SetProfile`, `SetVideoTransform`, `SetAudioGain`, `SetAudioMute`, `Reconnect`, `Stop`. These are **proposed new commands**, not existing wire capabilities. A local command either changes receiver-owned DSP/settings or is translated to an acknowledged phone configuration transaction.

Every command returns requested, effective, reason/error code and applied generation/config ID. Coalesce rapid slider changes. No overlapping camera/config transactions. Reject stale acknowledgements; retry only idempotent operations with the same request ID. Local gain and flip must not restart capture. Preserve current output selection and connection preferences across restarts.

GUI shows source/lens, supported resolution/FPS, profile, effective codec/rate, GPU/CPU decoder, live output FPS, health and recovery reason. Controls are disabled with a reason until capabilities exist. Provide gain slider, mute, clipping meter, horizontal mirror, vertical flip, preview/webcam toggles and advanced output-format settings. Keep unsupported settings out of ordinary choices. Validate the desktop icon in app menu, desktop launcher, X11 and Wayland/Dock association; use a matching desktop application ID where needed and avoid duplicate launcher entries.

**Exit gate:** Linux controls work during a real session without full receiver restart, UI remains responsive during a 2-second slow operation, one output owner survives reconnect, and reported effective settings match the phone.

## 5. P1 — front and rear cameras, all supported modes

Apple lists front and rear iPhone 13 video recording up to 4K60 [S1]. That does not prove every AVCapture format/codec/HDR combination is usable in this app. Enumerate `AVCaptureDevice.DiscoverySession`, device formats and frame-rate ranges [S2]. Include front camera and rear wide/ultrawide devices when discovered; do not invent telephoto, ProRes, Apple Log or simultaneous multicamera support for ordinary iPhone 13.

Build a capability intersection: sensor format × delivered pixel-buffer dimensions × encoder capabilities × receiver decode × consumer output. Report actual lenses, dimensions, rational FPS ranges, SDR/HDR state and supported codec profiles. Catalog modes beyond 60 FPS when exposed, but label high-speed streaming as a separately qualified extension; current validators cap FPS at 60 and dimensions at 3840×2160. Do not expand bounds without resource/protocol tests. Still-photo resolution is not a live video mode.

Implement a camera switch transaction: pause new media, snapshot previous input/config, remove old input, add requested input, select format under lock, apply SDR policy, rebuild encoder as needed, advance config generation, deliver config/parameter sets, wait for acknowledgement, force IDR and resume. Roll back on any failure. Retain microphone input where possible, explicitly mark timeline discontinuity if a reset is unavoidable. Resolve queued callbacks from the old generation safely.

The current cached `camera` and `configured` flag cannot merely be changed to `.front`: the existing AVCaptureDeviceInput must also be replaced. Camera identity belongs in `sameMediaFormat`/configuration equality and compatibility tests. Return a supported fallback with the reason; never present a silently substituted mode as the requested one.

**Exit gate:** front and each discovered rear lens support all three profiles where the actual format intersection permits; test 50 alternating switches, unsupported mode rejection, reconnect, permissions and rollback. 4K60 requires physical qualification on each lens, not a specification-table assertion.

## 6. P1 — audio gain, mute and sync

Implement receiver digital gain after Opus/PCM decoding and before PipeWire publication, independently of the phone's hardware mic gain. Proposed range: −60 dB to +12 dB, default 0 dB, separate mute; linear multiplier `10^(dB/20)`. Process in float or sufficient-width arithmetic, smooth gain over a bounded 10–40 ms ramp, prevent wraparound, count clipping and provide peak/RMS metering. Optional limiter has an explicit latency budget and remains off by default. Gain cannot recover already clipped phone samples.

Mute publishes timed silence and keeps the virtual microphone available. Avoid toggling PipeWire nodes or rebuilding video for volume changes. Publish source-volume controls for desktop compatibility where supported, with one documented effective gain path to prevent accidental double amplification. Level meters and UI messages stay outside the PipeWire real-time callback [S8].

`AVAudioSession` sample rate/buffer duration are preferences; read effective route properties after activation [S6]. Add bounded conversion for non-48 kHz routes instead of the current blanket failure. Stereo is exposed only when the selected route/capture output delivers distinct supported channels. Optional remote hardware gain is capability-gated by `isInputGainSettable`; software gain is always the normal desktop control.

Replace stateless per-packet resampling with a stateful, tested drift correction preserving interpolation/filter history and fractional phase. Bound correction to the designed ppm range and use a single clock estimator with offset, drift and uncertainty. Do not infer sync from arrival order. PLC accounts for exact missing durations; prolonged gaps use bounded concealment then silence/recovery.

**Exit gate:** mute/unmute and gain sweeps produce no clicks, overflow or reconnect; expected amplitude verified with deterministic tones; ±300 ppm synthetic drift passes, and measured A/V skew p95 ≤30 ms for a healthy 60-minute session.

## 7. P1 — mirror/flip without lowering fidelity

Define separate horizontal mirror and vertical flip controls, plus explicit rotation/orientation. Preserve original dimensions, pixel aspect, range and color metadata. Default outgoing front-camera image is unmirrored; a user's selected mirror is explicit and persists per camera. Do not accidentally mirror twice through iOS automatic mirroring and receiver processing.

Prefer one receiver transform before the raw-output tee so preview and virtual camera agree. GPU transform requires an available compatible plugin and supported caps; probe instead of naming an assumed CUDA element. If webcam needs CPU memory, evaluate transform there against a shared GPU transform/download path. Use CPU `videoflip` as measured fallback when available. Do not transcode compressed media just to flip pixels.

**Exit gate:** asymmetric chart/text tests cover horizontal, vertical, both and rotations on front/rear sources; output shape and color remain correct; transformed 4K60 must meet the same smoothness gates. Preview-only mirroring, if offered later, is clearly separate.

## 8. P2 — Wi-Fi technology and network efficiency

| Option | Strength | Cost/constraint | TitanCam decision |
|---|---|---|---|
| Network.framework TCP | Reliable framing, easy USB/control | Head-of-line delay on loss; freshness requires application bounds | Keep control and USB; optional explicit Wi-Fi compatibility mode only |
| Network.framework UDP | Public API; timely LAN media and flexible packet deadlines | App must own pacing, congestion response, repair, loss and clock handling | Improve current path first |
| QUIC streams + DATAGRAM | TLS security, congestion control; DATAGRAM avoids media retransmission waiting [S9] | TLS mandatory; reliable streams still order bytes; congestion-limited loss still possible; interop must be tested | Architectural alternative only; conflicts with current plaintext product contract |
| SRT | Mature UDP live transport, ARQ and timestamp-based delivery [S10] | Configured recovery latency/buffers; not automatic lower delay; library integration | Benchmark candidate for a stability-oriented higher-delay option |
| RIST | Interoperable broadcast contribution and retransmission [S11] | Recovery window, extra traffic, library/interop work | Evaluate for lossy contribution use; no assumed LAN advantage |
| Native WebRTC | Mature media feedback/jitter/loss handling and browser integration [S12] | DTLS/SRTP, signaling/ICE, dependency complexity, 4K60 needs independent qualification | Future secure/browser product option; borrow concepts, not a partial home-grown WebRTC claim |

A new transport cannot fix slow exposure, blocked media worker, GPU copies or consumer buffering. Compare candidates with identical codec, image quality, impairment and playout budget.

Implement in order:

1. Feedback deltas over a fixed window for wire bytes, loss, queue age and delivery timing; detect stale feedback. Use delay trend plus loss, not only expired units. Pacing is not congestion control. Receiver output overload and network overload are different signals.
2. Adaptive ceiling ≤70% of measured sustainable payload capacity as an initial policy, with all audio, headers, repair and FEC inside the wire budget. Use passive delivery samples or bounded startup probes, not saturating background speed tests. Downshift quickly; increase slowly after stable windows with hysteresis. Keep bitrate-only changes on the existing encoder session. Expose reason and ceiling on Linux.
3. Preserve 1100-byte datagrams initially. At 65 Mbit/s, 1036-byte payloads produce approximately 7,843 packets/s before audio/repair, and 64-byte media headers alone add about 6.2% overhead; IP/UDP/Wi-Fi add more. Measure packet processing load and IDR bursts. Raising MTU needs path tests and no fragmentation assumption.
4. A completed-frame deadline includes sender queuing and receiver timing; a fixed 60 ms timer starting at first fragment is not a complete capture-to-display budget. Make deadlines profile/path-aware within hard bounds. Discard obsolete repair work.
5. Selective NACK/repair: bounded packet cache by age/bytes, fragment bitmap with strict limits, rate-limited request batches, duplicate suppression, session/config validation. Resend only if estimated RTT + decode margin fits the remaining presentation deadline and total wire budget. Do not invent reliable delivery of every frame.
6. Rate-limit recovery storms and avoid losing each oversized IDR to the same bandwidth/expiry cycle. Measure IDR size and pacing completion; tune keyframe cadence/burst budget and encoder rate limits only when supported [S3]. Never require unsupported properties.
7. Optional adaptive FEC is an experiment, disabled initially. Compare approximately 0/5/10% overhead under random and burst loss, with hard aggregate bitrate bounds. Enable only when reduced freezes justify overhead; avoid FEC plus unrestricted ARQ amplification.
8. Wi-Fi 5/6 capability does not mean the app can choose AP channel/PHY width or force QoS. Recommend a clean 5 GHz path and wired PC-to-AP where available; report topology and test client isolation, roaming, multicast discovery, IPv4/IPv6 and multi-interface choices.

**Exit gate:** loss 0/0.1/1/3%, burst gaps 50/100/250 ms, RTT 2/10/30 ms, jitter 0/5/20 ms, reordered/duplicate packets, and capacity steps above/below target; healthy mode meets latency/FPS gates, impaired modes degrade visibly without growing queues. Reports include total wire rate and recovery traffic, not encoded bitrate alone.

## 9. Operating modes and quality policy

These are proposed starting ranges, not measured guarantees. Support manual resolution/FPS bounded by camera capabilities within each profile. Keep requested settings distinct from effective settings and automatic safety downgrade.

| Mode | Starting point | Proposed video bitrate envelope | Audio | Controller policy |
|---|---|---|---|---|
| Power Saver | 720p30 H.264; optional 1080p30 after energy comparison | 2–6 Mbit/s H.264 | Opus mono 48 kHz, 20 ms, 64–96 kbit/s on Wi-Fi; PCM on USB | Preview off option, slower UI updates, favor lower capture load |
| Balanced | 1080p60 H.264 initially; HEVC if measured better | H.264 8–18 or HEVC 6–14 Mbit/s | Opus 10 ms, 96–160 kbit/s; true stereo optional | Smooth motion, adaptive rate, deliberate thermal downgrades |
| Max Quality/Performance | 4K60 SDR, HEVC Main candidate; H.264 compatible fallback | HEVC 25–55 or H.264 45–80 Mbit/s | Opus 5/10 ms, 128–256 kbit/s if true stereo supported; PCM USB | Require adequate measured capacity/decoder/thermal state; offer 4K30 or 1080p60 downgrade |

H.264's dedicated VideoToolbox low-latency rate-control mode has a documented codec restriction [S3]. Do not assume it applies to HEVC. Require hardware encoding, disable frame reordering, verify setters and preserve color metadata. Inspect encoder output/capabilities before promising HEVC Main10/HDR. SDR remains default for webcam compatibility; HDR requires end-to-end 10-bit capture/encode/decode/output and explicit SDR mapping where necessary.

Evaluate compression using paired raw input and decoded output at identical timing/dimensions: SSIM/PSNR and optionally VMAF, plus motion, hair, skin, fine text, gradients and low-light inspection. HEVC may save bandwidth at similar perceived quality; it does not create missing sensor detail. Sharpen/denoise enhancements are optional measured later work, never silently applied. Reusing a codec's name or increasing bitrate alone is not proof of quality.

Observe `ProcessInfo.thermalState` and camera `systemPressureState` [S4]. Hysteresis prevents mode oscillation. Serious pressure reduces capture cost, not merely network bitrate: FPS/resolution/HDR/processing. Critical/shutdown ends capture safely with a visible reason. Increase frame duration to lower FPS. Restoration waits for sustained recovery, then negotiates a new config. No indefinite 4K60 or battery-saving claim without measurement.

## 10. Latency, smoothness and recovery acceptance

Zero latency is physically impossible: exposure, frame sampling, encoding, transport, decoding, output buffering and display scanout take time. Targets below assume adequate illumination, low host load, a healthy LAN and a ≥60 Hz display; they are unverified engineering budgets.

| Measurement | USB goal | Healthy LAN/Wi-Fi goal |
|---|---:|---:|
| 1080p60 glass-to-glass median | 45–80 ms | 60–110 ms |
| 1080p60 glass-to-glass p95 | ≤120 ms | ≤160 ms |
| 4K60 experimental p95 | ≤150 ms | ≤200 ms |
| A/V absolute skew p95 | ≤30 ms | ≤30 ms |
| Effective output at nominal 60 FPS | ≥59 FPS averaged over 10 minutes | ≥59 FPS averaged over 10 minutes |
| Healthy-session freeze >250 ms | 0 in a 60-minute qualification | 0 in a 60-minute qualification |
| Recovery from isolated reference loss | ≤300 ms target | ≤300 ms target |
| Usable media after link is restored and phone ready | ≤5 seconds target | ≤5 seconds target |

For 1080p60 target approximately 20 ms capture/exposure scheduling, 15 ms encode, 15 ms USB or 30 ms LAN transfer/reassembly, 10 ms decode/transform, 20 ms output/display; remaining p95 allowance covers jitter. These are overlapping-stage design allowances, not a claimed measured sum. Thirty FPS and longer low-light exposure need a different budget. Record p99 and all failures, not just medians. Consumer OBS/browser delay is measured separately.

Measure a physical flashing target and the receiving display with a high-speed camera, with synchronized acoustic clicks for sync, ≥1000 samples across the sustained session, exposure/refresh and uncertainty stated. Internal clocks establish stage latency with offset uncertainty, not sensor-to-display latency. Frame counters must distinguish queue acceptance from actual sink/consumer delivery.

Fault tests: 50 USB replug cycles, phone background/foreground and lock/unlock, phone call and audio-route change, receiver crash/restart, AP loss/address change, GPU unavailability, PipeWire restart, missing module/device permissions, slow webcam consumer, denied camera/mic/LAN permission, and concurrent commands. Stale sessions/configs never regain output ownership. Backoff is bounded and jittered; do not retry denied permission or incompatible protocol forever.

## 11. iOS background and screen power

The existing app is a foreground camera sender and explicitly stops in `.background`. Ordinary background camera capture is restricted [S5]. Public multitasking/PiP support is capability-, entitlement- and use-case-dependent and is not a promise that this sender continues filming with the iPhone locked. Do not add fake VoIP/audio background activity or undocumented APIs.

Implement a foreground “Dim screen” mode: reduce brightness after explicit user activation, minimal dark status screen, no preview, tap to restore. Save/restore prior brightness and idle-timer state on stop, exit and interruption. iPhone 13 has an OLED display but a black screen is not screen-off or guaranteed negligible power. Lock/background displays a paused reason on Linux; foreground return safely renegotiates and requests fresh media, subject to permissions/device availability. Audio-only background is a separate opt-in product feature, not a loophole for video.

## 12. Error handling, debugging and cleanup

Define error domains/codes for permissions, format/encoder, transport timeout, USB enumeration/trust, protocol mismatch, decoder/driver, output negotiation, PipeWire, thermal and configuration rollback. Errors carry stage, retryability, recovery action and cause; user messages stay understandable. Preserve GStreamer bus error/debug details in bounded local logs. Avoid broad `try?`/catch-and-ignore for operations that determine effective state; expected shutdown errors are classified separately.

Use structured rotated logs, generation/request IDs, effective-config dump, pipeline graph/caps export and a redacted diagnostic bundle. Collect rolling counters without storing camera/audio. Raw-media recording is explicit opt-in. Do not put UDIDs, Apple credentials or private phone crash logs in ordinary artifacts. Fixed-size real-time telemetry leaves PipeWire callbacks through atomic/SPSC data; logging happens elsewhere.

Audit cancellation/join paths and FFI lifetime/thread ownership. Confirm worker failure cancels the session and releases its output permit; every socket/queue/timer has a finite shutdown path. Replace dense one-line critical state transitions with readable typed code after regression coverage. Remove stale pairing/TLS comments from active local-v2 docs/code without discarding historical research.

## 13. Build, test and installation plan

Retain current Linux Ubuntu 24.04/26.04 amd64 matrix and locked toolchain; test package dependencies/ABI separately. Build format/lint/unit/protocol tests, shared Swift/Rust golden fixtures, GOP loss/reorder/repair fixtures, clock/drift, gain/transform/config rollback tests and synthetic media benchmarks. Add GUI syntax/import checks, desktop-file validation, brand-asset presence and clean-package installation smoke checks. Do not write tests that only restate UI strings.

Keep macOS/Xcode iOS builds; Linux servers cannot substitute for Apple's SDK. Current workflow pins macOS 15/Xcode 16.4; check runner image availability before changing pins [S13]. Device compilation plus simulator tests cover code and pure policy, not sensor/USB/NVDEC/thermal performance. GPU/kernel/physical-iPhone testing is local or on labelled trusted self-hosted machines, never untrusted pull-request code.

Add capability/config/gain/flip protocol fixtures to both languages. Compatible optional additions negotiate feature support; required breaking changes bump transport/protocol compatibility and ship matched endpoints. Never reinterpret the existing v1 binary header or pretend local-v2 supports a future repair message without negotiation.

Release only after exact-source successful builds and hardware gates. Include matching DEB/IPA, hashes, dependency/license manifest, known limitations, upgrade/rollback instructions and qualification report. Unsigned IPA is **for local re-signing**, not directly installable. Verify the user's installed iLoader version/help and official supported route; bundle ID, entitlements, provisioning, expiry/refresh, trust and Developer Mode must match. No invented sideload commands or private signing APIs. Optional CI signing uses protected secrets, an ephemeral keychain and cleanup; credentials never enter fork/PR jobs [S14]. Keep remote publish/install separate from completion of this plan.

## 14. Delivery sequence

| Phase | Deliverable | Release gate |
|---|---|---|
| P0 | Counters, stall diagnosis, independent schedulers, baseline GPU report | Reproducible cause and improvement; no growing backlog |
| P1a | Local control API and responsive GUI | Acknowledged live commands, stable output ownership |
| P1b | Camera catalog/switch, gain/mute/meters, mirror/flip | Physical front/rear tests, DSP/transform regressions |
| P1c | Typed recovery, clock/drift correction, dim-screen mode | Interruption/reconnect and A/V qualification |
| P2a | Delay/loss adaptation and bounded selective repair | Impairment matrix within total wire budgets |
| P2b | HEVC/4K60/thermal/quality qualification | Sustained per-lens evidence; honest fallback |
| P3 | Packaging, cleanup, documentation and release evidence | Matching artifacts, clean install and local sideload |

Implementation update: 0.3.0 includes live camera/microphone controls, digital audio controls/meters, one-decode transforms, independent A/V workers, clock drift mapping, bounded selective repair/reordering, and a responsive process supervisor. CI builds and synthetic checks are recorded in docs/VALIDATION_0.3.0.md. Physical front/rear, 4K60, USB/Wi-Fi, color/quality, latency and two-hour qualification remain pending as requested by the owner. Adaptive FEC, HDR, capacity probing and recording remain deferred experiments.

## 15. Primary research sources

Checked 2026-10-01. API presence is evidence of a capability, not a TitanCam performance result.

- **S1:** [Apple iPhone 13 specifications](https://support.apple.com/en-gb/111872).
- **S2:** [AVCaptureDevice formats](https://developer.apple.com/documentation/avfoundation/avcapturedevice/formats).
- **S3:** [Apple low-latency VideoToolbox encoding](https://developer.apple.com/videos/play/wwdc2021/10158/) and [AverageBitRate semantics](https://developer.apple.com/documentation/videotoolbox/kvtcompressionpropertykey_averagebitrate).
- **S4:** [Camera system pressure](https://developer.apple.com/documentation/avfoundation/avcapturedevice/systempressurestate-swift.property).
- **S5:** [Background camera interruption](https://developer.apple.com/documentation/avfoundation/avcapturesession/interruptionreason/videodevicenotavailableinbackground), [multitasking camera capability](https://developer.apple.com/documentation/avfoundation/avcapturesession/ismultitaskingcameraaccesssupported), [Apple camera multitasking guidance](https://developer.apple.com/videos/play/wwdc2022/110429/).
- **S6:** [Audio session preferences versus actual hardware state](https://developer.apple.com/library/archive/qa/qa1631/_index.html), [input gain capability](https://developer.apple.com/documentation/avfaudio/avaudiosession/isinputgainsettable).
- **S7:** [GStreamer NVDEC H.264](https://gstreamer.freedesktop.org/documentation/nvcodec/nvh264dec.html), [HEVC](https://gstreamer.freedesktop.org/documentation/nvcodec/nvh265dec.html).
- **S8:** [PipeWire streams, source direction and real-time restrictions](https://docs.pipewire.org/devel/page_streams.html).
- **S9:** [Network.framework QUIC options](https://developer.apple.com/documentation/network/nwprotocolquic/options), [Apple QUIC/datagram networking](https://developer.apple.com/videos/play/wwdc2022/10078/).
- **S10:** [SRT API and transport latency/retransmission controls](https://github.com/Haivision/srt/blob/master/docs/API/API-socket-options.md), [Haivision latency guidance](https://doc.haivision.com/SRT/1.5.3/Haivision/latency).
- **S11:** [RIST Forum technical rationale](https://www.rist.tv/articles-and-deep-dives/2020/11/25/why-rist).
- **S12:** [WebRTC native APIs](https://webrtc.github.io/webrtc-org/native-code/native-apis/).
- **S13:** [GitHub runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
- **S14:** [GitHub Xcode signing workflow](https://docs.github.com/en/actions/how-tos/deploy/deploy-to-third-party-platforms/sign-xcode-applications).
- **S15:** [libusbmuxd ownership/daemon/iproxy and licensing](https://github.com/libimobiledevice/libusbmuxd); public iOS socket APIs are used by the app, but usbmux itself is an independent reverse-engineered host implementation, not a documented Apple accessory API.
- **S16:** [v4l2loopback compatibility, exclusive caps and Secure Boot](https://github.com/v4l2loopback/v4l2loopback). Kernel-module setup is separate from the normal-user receiver; signing the module is preferred to disabling Secure Boot.
