# Validation evidence — development preview, 2026-10-01

These checks are build/synthetic evidence, not physical-iPhone qualification.

| Check | Result / scope |
|---|---|
| Rust format + strict clippy | Passed locally; GitHub Ubuntu 24.04 also runs them |
| Rust unit tests | 8 passed: wire bounds/reassembly, golden header, partial records, pin rejection, replay/reflection, resample channel preservation |
| Encrypted integration | Real TLS + Quinn QUIC on localhost, generated H.264 reference GOP, 90 source frames, deliberately omitted frame 5 |
| Reference-loss accounting | 1 missing unit, 24 dependent frames discarded, 65 frames submitted to decoder; 3 recoveries covering startup, missing reference and configuration change |
| Configuration synchronization | MediaReady, StreamingReady and ConfigureApplied barriers exercised; no new-config media precedes receiver application |
| NVIDIA encrypted path | Actual receiver appsrc/parser/NVDEC pipeline exercised with the synthetic GOP; 61 units submitted, 28 discarded (includes deliberate loss and cold start), 3 recoveries; not a steady-state performance benchmark |
| Hardware video probe | 30 generated 1280×720 H.264 frames decoded by nvh264dec on the development RTX 4050 Laptop |
| Native mic consumer | Generated 440 Hz tone published as native PipeWire Audio/Source and received by pw-record: 165,888 samples, 119,591 above threshold; queue drained to zero |
| Mic startup underruns | 175 in the standalone tone experiment, which includes intentional startup silence/no production feed; not a steady streaming benchmark |
| Website | GitHub Pages deployed, HTTP 200, rendered layout inspected |
| iOS | Device compilation on GitHub macOS/Xcode; 7 simulator tests passed, including cross-language golden bytes, role/nonce binding, pre-auth command gating and real Keychain persistence; latest successful Actions run is the evidence |

Raw camera/audio from the user is never recorded by these tests. The microphone experiment
records the named synthetic source only. Temporary test identities/tokens remain local and
are removed; public reports contain counters, not secrets or device IDs.

Not established: actual AVFoundation/VideoToolbox operation on the phone; native Apple QUIC
interoperability; USB app listeners through Lightning; OBS/browser A/V sync; color accuracy;
clean-host package installation; continuous 4K60; measured glass-to-glass latency or energy.
Use the acceptance matrix in ARCHITECTURE.md before declaring a stable release.
