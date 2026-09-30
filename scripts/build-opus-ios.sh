#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -s)" == Darwin ]] || { echo 'iOS builds run on the macOS GitHub Actions runner.' >&2; exit 1; }
mkdir -p build
if [[ ! -d build/opus-source/.git ]]; then git clone --quiet https://github.com/xiph/opus.git build/opus-source; fi
git -C build/opus-source checkout --quiet --detach a5d6c1b6f4e582df97390f9ac5c6e7c51cbffffe
[[ "$(git -C build/opus-source rev-parse HEAD)" == a5d6c1b6f4e582df97390f9ac5c6e7c51cbffffe ]]
mkdir -p build/opus-headers/opus
cp build/opus-source/include/*.h build/opus-headers/opus/
for item in 'iphoneos arm64 device' 'iphonesimulator arm64 simulator'; do
  read -r sdk arch name <<< "$item"
  cmake -S build/opus-source -B "build/opus-$name" -DCMAKE_SYSTEM_NAME=iOS -DCMAKE_OSX_SYSROOT="$(xcrun --sdk "$sdk" --show-sdk-path)" -DCMAKE_OSX_ARCHITECTURES="$arch" -DCMAKE_OSX_DEPLOYMENT_TARGET=16.0 -DCMAKE_BUILD_TYPE=Release -DOPUS_BUILD_TESTING=OFF -DOPUS_BUILD_PROGRAMS=OFF -DBUILD_SHARED_LIBS=OFF
  cmake --build "build/opus-$name" --parallel 3
done
rm -rf build/Opus.xcframework
xcodebuild -create-xcframework -library build/opus-device/libopus.a -headers build/opus-headers -library build/opus-simulator/libopus.a -headers build/opus-headers -output build/Opus.xcframework
