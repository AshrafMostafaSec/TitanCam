# Local v2 implementation and acceptance

0.2.0 replaces authentication/TLS/QUIC with clear TCP control/USB and paced UDP media,
as explicitly requested for the owner's trusted LAN. Bonjour and bounded USB metadata
automatically discover endpoints. The iPhone UI is sender-only without preview;
Linux starts immediately, controls profile/outputs and retains one active media owner.
NVDEC decodes once; webcam uses native-size NV12 and optional GL preview is bounded.

Verification is recorded in Actions and the release notes. Unit/synthetic/simulator
checks are not a real camera test. Install the matching new IPA through local iLoader
before attempting to connect; alpha.1 is incompatible.

Required hardware qualification:
1. Confirm actual phone capture, hardware encode and effective saver format.
2. Ten-minute USB capture and unplug/reconnect, plus immediate Stop/Start.
3. Discover the computer on Wi-Fi, stream and interrupt/reconnect the AP.
4. Validate NVDEC, webcam and microphone in OBS/browser, flash/click A/V skew.
5. Measure sustained profiles, GPU decode/power, thermal/battery and glass-to-glass delay.

Maximum 4K60/HEVC, exact latency, long-session stability and quality/energy targets
remain unqualified. Repair/FEC, front camera/HDR, IPv6/interface selection and recording
are later work. No media recording/upload is enabled.
