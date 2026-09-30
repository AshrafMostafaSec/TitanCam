# Development host observations — 2026-10-01

| Component | Observed |
|---|---|
| Linux | Ubuntu 26.04.1 x86_64; kernel 7.0.0-34 |
| GPU | RTX 4050 Laptop; NVIDIA driver 595.91.07 |
| GStreamer | 1.28.2; nvh264dec/nvh265dec available |
| Decode probe | 30 generated H.264 frames actually decoded using nvh264dec |
| PipeWire | 1.6.2; user session active; native Audio/Source created |
| USB | usbmuxd running; one phone enumerated; device ID withheld |
| Rust/Cargo | Verified 1.98.1 project-local toolchain |
| Native headers | GStreamer/PipeWire/usbmuxd/Opus installed |
| iLoader | Installed package 2.3.4; local graphical signing route |
| Apple SDK | No local macOS/Xcode; GitHub macOS build workflow |

Eight Rust tests and strict lint passed. Release compilation and GitHub jobs provide
additional build evidence; see the actual Actions run for final status. PipeWire consumer
playback, iPhone camera encoding, USB app-port streaming, native iOS QUIC interoperability,
virtual webcam/browser integration and sustained latency/quality/energy remain hardware gates.
No camera recordings, credentials, device identifiers or pairing tokens are published.
