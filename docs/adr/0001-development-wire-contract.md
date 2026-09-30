# ADR 0001 — development v1 wire contract and readiness barriers

Accepted for the development preview, 2026-10-01. The main architecture describes the
production target. This ADR states the exact implemented subset and deviations; do not
infer unimplemented features from that target.

Control framing and 64-byte media header match ARCHITECTURE section 7. All complete video
access units are Annex B with parameter sets repeated on independent frames. Separate reliable
CodecConfig/bitstream-validation transactions remain a qualification/release gate. SDR uses
limited-range NV12 and propagates source primaries/transfer/matrix into the encoder.

Connection order:
1. TLS (ALPN `titancam-control/1`); explicit receiver certificate pin on Wi-Fi, phone pin on USB.
2. Receiver Hello: random session, nonce, receiver Ed25519 key and random media token.
3. Phone AuthProof: Ed25519 role-1 signature binding both identity hashes, nonce and session.
   Wi-Fi pairing token is entered on the phone and never echoed in receiver Hello. For USB,
   the locally entered phone token is sent inside the pinned TLS channel. Tokens last 120 s.
4. Receiver verifies known identity or pairing token; returns role-2 AuthOk proof.
5. Configure → ConfigureAck; receiver installs effective configuration and registry entry.
6. **MediaReady** is a reliable barrier. Only then does the phone initiate media binding.
7. Wi-Fi: native QUIC datagram flow (`titancam-media/1`), 57-byte TCMB binding. USB: two
   separate TLS connections with MediaBind token and channel role. Each role is accepted once.
8. Start → StartAck(host_epoch_ns) → **StreamingReady**. Capture starts after this barrier;
   sending an IDR immediately after writing StartAck is unsafe across independent transports.
9. On thermal/network reconfiguration, capture pauses; ConfigureAck → **ConfigureApplied**
   resumes capture with an IDR. The control acknowledgement must precede new-config media.

The barriers were added after the synthetic peer reproduced a control/QUIC startup race.
TCP/TLS writes are explicitly flushed. A persistent control reader preserves partially read
records across feedback/clock timer events; repeatedly cancelling read_exact is incorrect.

A new reconnect creates a fresh session, epoch 1. In-place transport switching/epoch commits
are not implemented. Sequence numbers persist across configuration changes within a session;
receiver resets decoder/reference and audio codec state at a new config ID. iOS requires 17.4
or newer so public required-hardware and hardware-status encoder APIs are available.

Binding expires within 10 seconds of session creation in this preview. Repeat binding is
ignored on the already authenticated connection; the token cannot bind a second simultaneous
connection. The control peer reconnects after media failure. Planned 5-second token minting
at media readiness, idempotent acknowledgement retry and comprehensive revocation of existing
sessions are further security hardening gates.

Bounds: control 64 KiB, nesting 16, handshake 5 s, server 4 concurrent control and 4 media
connections, reassembly 4 incomplete AUs/32 MiB/60 ms, video AU 8 MiB, audio 64 KiB,
fragments <=16384, receiver ingress 16 units/16 MiB, commands 32, app encoder 2 in-flight frames,
USB media 2 pending records/channel, QUIC 2 queued video AUs/10 audio packets/8 sends.
Stale QUIC video is discarded at 100 ms with IDR recovery. Current reassembly timeout is
fixed; adaptive jitter/repair/FEC and calibrated drift fitting are not enabled.

Native datagram-flow and timeout API semantics were checked against
[Apple QUIC options](https://developer.apple.com/documentation/network/nwprotocolquic/options).
Rust↔Rust localhost QUIC tests do not establish Apple↔Quinn physical-device interoperability.

Decoder recovery flushes buffered compressed data while retaining the live decoder/GPU
context. Same-codec reconfiguration reuses the pipeline; same-channel audio retains its
PipeWire source node. Destroying those resources on every bitrate update adds cold-start
delay and can disconnect microphone consumers. Codec state/reference tracking still resets
at the configuration barrier. CPU fallback telemetry names the decoder actually selected.

## USB endpoint lifecycle revision (alpha.2)

Physical alpha.1 testing reported NWError POSIX 48 (address already in use). The exact competing socket was not observed. Fixed-port reuse and asynchronous cancellation were both weaknesses: repeated Enable taps cancelled and immediately recreated three listeners, advertised readiness before bind completion, and allowed retired listeners' failures to affect a new transport.

Alpha.2 uses the triplets in `protocol/usb-ports-v1.json`, outside the usual high ephemeral-port range. It waits for all listener `.ready` events before displaying the temporary token, ignores stale-generation callbacks, awaits `.cancelled` before restart/switch, permits local endpoint reuse, and falls back to another complete triplet on EADDRINUSE. TLS pinning and mutual identity proof remain required. The receiver's `--usb-base-port` selects the phone's displayed base; `49152` remains selectable for alpha.1 phones. Media framing/authentication are unchanged. Simulator tests exercise repeated start, immediate stop/restart, a deliberately occupied port and USB→Wi-Fi release. Physical-phone retesting is still required.
