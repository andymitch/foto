#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build -q -p foto-cli
bin=$PWD/target/debug/foto
t=$(mktemp -d)
trap 'kill $(jobs -p) 2>/dev/null; rm -rf "$t"' EXIT
mkdir -p "$t/src"
for i in 1 2 3; do
  ffmpeg -v quiet -f lavfi -i "testsrc=size=1280x720:rate=1" -ss "$i" -frames:v 1 "$t/src/photo$i.jpg"
done
ffmpeg -v quiet -f lavfi -i "testsrc=size=640x360:rate=25" -t 3 -pix_fmt yuv420p "$t/src/clip.mp4"
a() { FOTO_HOME=$t/a FOTO_PICTURES=$t/a-pics "$bin" "$@"; }
b() { FOTO_HOME=$t/b FOTO_PICTURES=$t/b-pics "$bin" "$@"; }
a create Demo >/dev/null
a add "$t/src"
ticket=$(a share)
FOTO_HOME=$t/a FOTO_PICTURES=$t/a-pics "$bin" run >/dev/null &
pa=$!
b join "$ticket" >/dev/null
FOTO_HOME=$t/b FOTO_PICTURES=$t/b-pics "$bin" run >/dev/null &
pb=$!
for _ in $(seq 60); do
  n=$(find "$t/b-pics" -type f 2>/dev/null | wc -l | tr -d ' ')
  [ "$n" -ge 4 ] && break
  sleep 1
done
kill $pa $pb
wait $pa $pb 2>/dev/null || true
b status
find "$t/b-pics" -type f
[ "$n" -ge 4 ] && echo "OK: $n files synced" || { echo "FAIL: $n files"; exit 1; }
