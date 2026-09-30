# Validation evidence — development preview, 2026-10-01

These checks are build/synthetic evidence, not physical-iPhone qualification.

| Check | Result / scope |
|---|---|
| Rust format + strict clippy | Passed locally; GitHub Ubuntu 24.04 also runs them |
| Rust unit tests | 8 passed: wire bounds/reassembly, golden header, partial records, pin rejection, replay/reflection, resample channel preservation |
| Encrypted integration | Real TLS + Quinn QUIC on localhost, generated H.264 reference GOP, 90 source frames, deliberately omitted frame 5 |
| Reference-loss accounting | 1 missing unit, 24 dependent frames discarded, 65 frames submitted to decoder; 3 recoveries covering startup, missing reference and configuration change |
| Configuration synchronization | MediaReady, StreamingReady and ConfigureApplied barriers exercised; no new-config media precedes receiver application |
| Hardware video probe | 30 generated 1280×720 H.264 frames decoded by nvh264dec on the development RTX 4050 Laptop |
| Native mic consumer | Generated 440 Hz tone published as native PipeWire Audio/Source and received by pw-record: 165,888 samples, 119,591 above threshold; queue drained to zero |
| Mic startup underruns | 175 in the standalone tone experiment, which includes intentional startup silence/no production feed; not a steady streaming benchmark |
| Website | GitHub Pages deployed, HTTP 200, rendered layout inspected |
| iOS | Device compilation on GitHub macOS/Xcode; simulator tests are a separate gate, use the latest successful Actions run |

Raw camera/audio from the user is never recorded by these tests. The microphone experiment
records the named synthetic source only. Temporary test identities/tokens remain local and
are removed; public reports contain counters, not secrets or device IDs.

Not established: actual AVFoundation/VideoToolbox operation on the phone; native Apple QUIC
interoperability; USB app listeners through Lightning; OBS/browser A/V sync; color accuracy;
clean-host package installation; continuous 4K60; measured glass-to-glass latency or energy.
Use the acceptance matrix in ARCHITECTURE.md before declaring a stable release.
