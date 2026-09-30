# iOS app

SwiftUI foreground app with AVFoundation camera/audio, required VideoToolbox hardware
encoding, native TLS/QUIC, Keychain identities and pinned receiver pairing.

Build on GitHub Actions, not on the user's Linux PC. `ios/project.yml` uses pinned
XcodeGen; the generated shared-scheme project and Swift package lock are exported by CI.
`build-ios.sh device-unsigned` produces an explicitly unsigned IPA for local re-signing.
`build-ios.sh simulator` runs protocol/security tests; it does not validate camera hardware.
See `docs/BUILD_AND_SIGNING.md` and `docs/NEXT_STEPS.md` for installation and qualification.
