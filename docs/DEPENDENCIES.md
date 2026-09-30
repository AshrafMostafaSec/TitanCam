# Dependency/distribution notes

Rust dependencies are locked in Cargo.lock. The Linux package exports cargo metadata
with every dependency version and SPDX license field; a standardized CycloneDX/SPDX SBOM
is still a release gate. Rust libraries use their upstream MIT/Apache/BSD/ISC licenses;
review the exported manifest before shipping a stable release. Sources and license notices
must accompany a binary distribution as required by each dependency's terms.

GStreamer and PipeWire are dynamically linked, installed by the distribution; GStreamer
LGPL libraries/plugins and optional codec plugin licensing require a packaging review.
libusbmuxd is LGPL; libopus is BSD-style. Opus 1.6.1 is statically linked into the iOS app;
include its upstream COPYING in distributed artifacts. Apple swift-certificates,
swift-asn1 and swift-crypto use Apache-2.0 and their upstream notices. Apple SDK/frameworks
are build/runtime dependencies under Apple's terms, not bundled open-source assets.

v4l2loopback is a separately installed GPL kernel module. NVIDIA drivers are proprietary,
installed separately; TitanCam packages do not redistribute their binaries.
