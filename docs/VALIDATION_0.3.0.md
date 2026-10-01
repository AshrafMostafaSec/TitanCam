# TitanCam 0.3.0 validation and handoff

Date: 2026-10-01. Development preview; physical two-hour testing is deferred at the owner's request. Update both endpoints. The device IPA is unsigned for local signing.

## Implementation delivered

- Linux live camera/lens/format and microphone/input/data-source controls with correlated acknowledgements, rollback and actual fallback reporting.
- H.264/HEVC Main SDR; Saver, Balanced and Maximum presets; actual camera capabilities up to 4K60 rather than invented model tables.
- Gain from -60 to +12 dB, timed-silence mute, level/clipping meters; horizontal/vertical transforms shared across preview/webcam with one decoder.
- Independent audio/video scheduling, affine clock drift mapping, stateful audio drift resampling, and non-48kHz phone-route conversion.
- Deadline-bounded UDP completion reordering and selective fragment repair, with bounded queues/cache and limited traffic inside the existing pacer; conservative bitrate adaptation.
- Foreground dim/restore, safe reconnect/source persistence within a receiver process, last disconnect reason, atomic statistics and actual decoder-output counters.
- Desktop artwork, responsive receiver shutdown/restart, updated Xcode project, matched version 0.3.0/build 7, build artifacts, schemas and operation/signing documentation.

## Reproducible checks

`source scripts/env.sh; scripts/check.sh` passes format, lint, 16 Rust tests and release compilation. `scripts/verify-protocol-fixtures.sh` passes shared wire bytes and schemas. The simulated clock test covers 7200 seconds at ±300ppm with asymmetric RTT outliers; it is not a two-hour camera session. Swift simulator CI covers 23 tests, including 44.1kHz-to-48kHz audio conversion, configuration correlation, parser/queue bounds and transport lifecycle. Exact-source final CI references accompany the downloadable handoff report.

GPU check command:

```sh
./target/release/titan-bench --hardware --repair --fps 60 --preview --webcam /dev/video42 --audio --controls
```

Local host: Ubuntu 26.04.1, GStreamer 1.28.2, PipeWire 1.6.2, RTX 4050 Laptop with driver 595.91.07. The test uses generated H.264 at **320×180/60 FPS over localhost**, not an iPhone, real Wi-Fi, 4K60 or a fidelity benchmark. Synthetic PCM tone is local only. Gain/mute and both transforms are exercised through the same private Unix API as GTK. A deliberately omitted fragment is requested and retransmitted. Original GPU initialization caused audio lateness; moving initialization off Tokio workers fixed it. Reapplying transform properties every frame was removed, and compressed ingress has explicit count/byte/time bounds.

| Last local combined-output run | Result |
|---|---:|
| Generated video / accepted to decoder / decoder output | 300 / 271 / 269 |
| Generated audio / processed audio packets | 499 / 499 |
| Fragment repair requests / assembly expirations | 1 / 0 |
| Dropped video/audio aggregate at warm-up baseline / final | 29 / 29 |
| Late video at baseline / final | 11 / 11 |
| Late audio at baseline / final | 0 / 0 |
| PipeWire underruns at baseline / final | 13 / 14 |
| Tone gain metering, final mute, mirror/flip | passed |

Initial GPU warm-up **still drops video** and requests fresh keyframes. The short steady-state assertions pass with no increased missing/expired/late/reference-discard counters after the recorded baseline. PipeWire startup/end gaps remain counted; do not reinterpret the aggregate underruns as measured sustained audio stability. The source probe counts decoded buffers, not displayed or OBS-consumed frames. Its reported FPS window can include the synthetic stream ending. JSON retains all counters; none are suppressed.

Healthy and repaired software fixtures run in Linux CI on both Ubuntu targets. They send 300 generated frames at 60FPS and check that completion repair does not trigger reference loss after warm-up. The separate recovery fixture intentionally drops a reference frame and changes configuration; its drops/recoveries are expected, not an uninterrupted-performance claim.

GTK construction/control mapping was checked in an isolated session with fixture capabilities, including actual mic data source vs system default and disabling capture controls when disconnected. A deliberately slow receiver process verified that stopping returns immediately on the UI thread while the main loop remains responsive. This is not visual/phone hardware acceptance.

The benchmark isolates its control socket while preserving the user's [PipeWire runtime directory](https://docs.pipewire.org/1.4/page_man_pipewire_1.html). It does not take over an active desktop receiver socket.

## Installation and remaining gates

Select the DEB for Ubuntu 24.04 or 26.04; keep host NVIDIA drivers and v4l2loopback setup separate. Use local iLoader to sign/provision/install the unsigned IPA. Credentials and phone installation stay local. No valid device signature or phone install is claimed by CI.

Required later: actual front/rear and microphone source controls; USB unplug/reconnect and Wi-Fi/AP interruption; OBS/browser webcam and microphone; real flash/click A/V sync and glass-to-glass latency; quality/colors; two-hour thermal/pressure/power tests per mode; 4K60/HEVC qualification. Zero latency is physically impossible. Current playout defaults are 35ms USB and 70/60/65ms Wi-Fi Saver/Balanced/Maximum, not measured end-to-end latency.

Ordinary locked/background camera capture is unsupported; foreground dimming is implemented. HDR, FEC, capacity probing and recording are deferred experiments. Receiver source choices survive transport reconnect, not receiver process replacement. The transport remains deliberately plaintext TCP control/USB plus paced UDP for the trusted LAN; QUIC/WebRTC are not silently introduced.
