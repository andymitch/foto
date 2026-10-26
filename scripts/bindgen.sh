#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
lang=${1:?usage: bindgen.sh kotlin|swift <out-dir> [library]}
out=${2:?usage: bindgen.sh kotlin|swift <out-dir> [library]}
lib=${3:-}
if [ -z "$lib" ]; then
  cargo build -q -p foto-ffi
  lib=target/debug/libfoto_ffi.dylib
  [ -f "$lib" ] || lib=target/debug/libfoto_ffi.so
fi
cargo run -q -p uniffi-bindgen -- generate --library "$lib" --language "$lang" --out-dir "$out" --no-format
