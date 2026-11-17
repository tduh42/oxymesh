#!/usr/bin/env bash
# Fails when the M5 companion image (the largest) nears the size of its app partition.
# Usage: scripts/size.sh [limit in percent, 90]   (after a release build of the firmware)
set -euo pipefail
cd "$(dirname "$0")/../firmware/esp32s3"
limit="${1:-90}"
elf=target/xtensa-esp32s3-none-elf/release/mc-m5
[ -f "$elf" ] || { echo "no $elf: build the firmware first" >&2; exit 1; }
tool=$(command -v xtensa-esp32s3-elf-size || true)
[ -n "$tool" ] || tool=$(ls "$HOME"/.rustup/toolchains/esp/xtensa-esp-elf/*/xtensa-esp-elf/bin/xtensa-esp32s3-elf-size 2>/dev/null | head -1 || true)
[ -n "$tool" ] || { echo "xtensa-esp32s3-elf-size not found (espup install --targets esp32s3)" >&2; exit 1; }
# text + data is what the image keeps in flash (espflash adds about 1 % of headers and padding).
read -r text data _ < <("$tool" "$elf" | tail -1)
part=$(awk -F, '$1 ~ /^factory/ { gsub(/ /, "", $5); print $5 }' partitions.csv)
[ -n "$part" ] || { echo "no factory partition in partitions.csv" >&2; exit 1; }
part=$((part))
used=$((text + data))
pct=$((used * 100 / part))
echo "mc-m5: $used bytes of $part ($pct %), limit $limit %"
if [ "$pct" -ge "$limit" ]; then
  echo "the image is too close to its partition" >&2
  exit 1
fi
