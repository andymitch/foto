#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
lang=${1:?usage: bindgen.sh kotlin|swift <out-dir>}
out=${2:?usage: bindgen.sh kotlin|swift <out-dir>}
cargo build -q -p foto-ffi
lib=target/debug/libfoto_ffi.dylib
[ -f "$lib" ] || lib=target/debug/libfoto_ffi.so
cargo run -q -p foto-ffi --features cli --bin uniffi-bindgen -- generate --library "$lib" --language "$lang" --out-dir "$out" --no-format
