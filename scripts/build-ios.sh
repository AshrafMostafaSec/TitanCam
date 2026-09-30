#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -s)" == Darwin ]] || { echo 'Use the iOS GitHub Actions workflow: this host has no Apple SDK.' >&2; exit 1; }
mode="${1:-device-unsigned}"
[[ -d build/Opus.xcframework ]] || ./scripts/build-opus-ios.sh
mkdir -p build/tools
if [[ ! -x build/tools/xcodegen/bin/xcodegen ]]; then
  curl --fail --location --retry 3 https://github.com/yonaskolb/XcodeGen/releases/download/2.46.0/xcodegen.zip -o build/tools/xcodegen.zip
  echo '4d9e34b62172d645eed6457cac13fc222569974098ef4ee9c3368bedf0196806  build/tools/xcodegen.zip' | shasum -a 256 --check
  unzip -q -o build/tools/xcodegen.zip -d build/tools
fi
build/tools/xcodegen/bin/xcodegen generate --spec ios/project.yml
common=(-project ios/TitanCam.xcodeproj -scheme TitanCam -configuration Release -derivedDataPath build/DerivedData CODE_SIGNING_ALLOWED=NO)
case "$mode" in
  simulator)
    destination="$(xcrun simctl list devices available -j | python3 -c 'import json,sys; d=json.load(sys.stdin); print(next(x["udid"] for k,v in d["devices"].items() if "iOS" in k for x in v if x["name"].startswith("iPhone")))')"
    xcodebuild "${common[@]}" -destination "id=$destination" test ENABLE_TESTABILITY=YES -resultBundlePath build/SimulatorTests.xcresult
    ;;
  device-unsigned)
    xcodebuild "${common[@]}" -sdk iphoneos -destination 'generic/platform=iOS' build
    mkdir -p build/unsigned/Payload
    cp -R build/DerivedData/Build/Products/Release-iphoneos/TitanCam.app build/unsigned/Payload/
    (cd build/unsigned && zip -q -r ../TitanCam-unsigned-for-local-resigning.ipa Payload)
    shasum -a 256 build/TitanCam-unsigned-for-local-resigning.ipa > build/IOS-SHA256SUMS.txt
    ;;
  *) echo 'Usage: build-ios.sh simulator|device-unsigned' >&2; exit 2;;
esac
