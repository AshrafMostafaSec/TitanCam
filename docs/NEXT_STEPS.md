# TitanCam 0.3.0 implementation and acceptance

The Linux GUI now discovers phone cameras/lenses and their supported formats; controls microphone inputs/data sources exposed by public AVAudioSession APIs; applies profiles and H.264/HEVC with correlated acknowledgements and explicit fallback; and offers gain/mute, meters, mirror and vertical flip. One decoder supplies both preview and webcam. A serialized background process supervisor keeps receiver restarts off the GTK main thread. The desktop icon uses the app artwork.

Independent video/audio workers share drift-aware sender clock mapping. Completed video AUs have bounded reordering; capability-negotiated UDP fragment repair uses deadline, cache, queue and aggregate traffic limits. Network adaptation lowers bitrate on expiry, missing/late units or elevated RTT, and restores it slowly; bitrate changes preserve the encoder. USB and Wi-Fi reconnect preserve the chosen source/configuration during a receiver process lifetime. Phone foreground restoration renegotiates after a background interruption. Dim screen is foreground operation, not locked-camera capture.

Install matching 0.3.0 endpoints. The iPhone archive is unsigned for local iLoader signing, not directly installable. See BUILD_AND_SIGNING.md and VALIDATION_0.3.0.md. Sources were built and tested; the owner explicitly deferred two-hour physical testing.

## Remaining hardware gates

1. Front/rear camera and actual built-in microphone-source selection, effective route reporting and denied/unavailable choices.
2. Real USB and Wi-Fi sessions; unplug/replug, Stop/Start, lock/unlock, foreground restoration, audio route interruption and AP outage.
3. NVDEC and image transforms with preview plus V4L2/PipeWire in OBS and a browser.
4. Recorded flash/click A/V skew, glass-to-glass latency and fidelity/color checks, with measurement uncertainty.
5. Two-hour operation, including 4K60/HEVC and H.264 comparisons, thermal/pressure behavior, sustainable network load and power.

Maximum 4K60/HEVC, exact latency, long-session stability and quality/energy targets remain unqualified. Ordinary locked/background camera capture is unsupported. HDR, FEC, passive capacity probing, IPv6/interface choice and recording are deferred work; no recording/upload is enabled. Settings survive transport reconnect, not receiver process replacement.
