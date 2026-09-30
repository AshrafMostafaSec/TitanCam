# Builds, signing and local sideloading

Linux receiver builds on Ubuntu 24.04 GitHub Actions. iOS builds on macOS 15 using
Xcode 16.4; app deployment target iOS 17.4 (hardware-encoder requirement APIs); all Apple compilation occurs on GitHub, never on the user's Linux PC.
XcodeGen 2.46.0 is checked against the publisher's SHA-256 digest. Opus 1.6.1 source
is checked out at an exact commit; X509/ASN1/Crypto package versions are pinned.
The generated Xcode project and package lock are exported in the iOS build artifact.

## Default: unsigned app for local re-signing

Download the iOS artifact after a successful build. The filename states its signing
status. An IPA is just a ZIP containing `Payload/TitanCam.app`; this unsigned archive
has no embedded valid development provisioning profile and is not directly installable.
Use the user's local iLoader (installed version 2.3.4 was detected) to import the IPA
and perform its supported local Apple-account signing/install flow. Official project:
https://github.com/nab138/iloader . Do not invent a CLI: the installed tool is graphical.

The tool needs to generate a matching app identifier, signing identity and profile.
Its selected bundle ID/entitlements, device registration and certificate/profile expiry
must match. Enable Developer Mode on iOS when required; trust the computer and follow
iLoader's on-device prompts. Personal-team restrictions/refresh intervals are imposed
by Apple and the signing route. The phone and Apple credentials are not sent to CI.

## Optional paid-team CI signing

Unsigned CI is the functioning default. To add signed development/ad-hoc exports,
provide a protected GitHub environment containing an Apple certificate `.p12`, its
password, team ID and matching provisioning profile with the actual phone registered.
An App Store archive does not substitute for development/ad-hoc sideload provisioning.

Implement the signed job only in trusted branch/tag workflows: import secrets into
an ephemeral keychain, install the profile, set the exact signing identity/team/profile,
archive with Xcode, export using a checked-in ExportOptions.plist for the chosen method,
and remove the keychain/profile under `if: always()`. Never expose these assets to PR
or fork jobs. No signing credentials were supplied; no CI-signed artifact is claimed.

## Release gates

CI checks and simulator tests establish build correctness, not iPhone hardware operation.
A stable release requires real USB/Wi-Fi sessions, camera hardware-encoder confirmation,
OBS/browser webcam and microphone checks, interruption/reconnect tests and the documented
latency/quality/energy qualification. Until then artifacts are labelled development previews.
