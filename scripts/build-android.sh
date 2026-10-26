#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export ANDROID_NDK_HOME=${ANDROID_NDK_HOME:-${NDK_HOME:-}}
export CARGO_PROFILE_RELEASE_STRIP=debuginfo
if [ "${1:-debug}" != release ]; then
  export CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
fi
cargo ndk -t arm64-v8a -t x86_64 -P 30 -o android/app/src/main/jniLibs build -p foto-ffi --release
find android/app/src/main/jniLibs -name "*.so" ! -name libfoto_ffi.so -delete
./scripts/bindgen.sh kotlin android/app/build/generated/uniffi android/app/src/main/jniLibs/arm64-v8a/libfoto_ffi.so
"$(ls -d "$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/*/bin | head -1)/llvm-strip" --strip-unneeded android/app/src/main/jniLibs/*/libfoto_ffi.so
