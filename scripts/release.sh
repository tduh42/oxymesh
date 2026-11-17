#!/usr/bin/env bash
# Builds every firmware image and gathers what a release publishes in <dir> (default: dist):
# one ELF per board and role (debug info stripped), the partition tables, SHA256SUMS, and
# NOTES.md (the release notes: what to flash and how).
# Usage: scripts/release.sh [dir]   (needs the Xtensa `esp` toolchain, like scripts/flash.sh)
set -euo pipefail
cd "$(dirname "$0")/.."
root=$(pwd)
dist="${1:-dist}"
case "$dist" in /*) ;; *) dist="$root/$dist" ;; esac
rm -rf "$dist"
mkdir -p "$dist"

fw=firmware/esp32s3
[ -f "$HOME/export-esp.sh" ] && . "$HOME/export-esp.sh"
(cd "$fw" && cargo build --quiet --release)

strip=$(command -v xtensa-esp32s3-elf-strip || true)
[ -n "$strip" ] || strip=$(ls "$HOME"/.rustup/toolchains/esp/xtensa-esp-elf/*/xtensa-esp-elf/bin/xtensa-esp32s3-elf-strip 2>/dev/null | head -1 || true)
[ -n "$strip" ] || { echo "xtensa-esp32s3-elf-strip not found (espup install --targets esp32s3)" >&2; exit 1; }

# Board names as scripts/flash.sh takes them: name, binary.
images=(
  "m5 mc-m5"
  "heltec-v4 mc-heltec-v4"
  "m5-repeater rpt-m5"
  "heltec-v4-repeater rpt-heltec-v4"
  "m5-room room-m5"
  "heltec-v4-room room-heltec-v4"
)
for i in "${images[@]}"; do
  read -r name bin <<<"$i"
  "$strip" --strip-debug -o "$dist/oxymesh-$name.elf" "$fw/target/xtensa-esp32s3-none-elf/release/$bin"
done
cp "$fw/partitions.csv" "$fw/partitions-heltec-v4.csv" "$dist/"
(cd "$dist" && shasum -a 256 oxymesh-*.elf partitions*.csv > SHA256SUMS)

commit=$(git rev-parse --short HEAD)
date=$(git log -1 --format=%cs)
cat > "$dist/NOTES.md" <<NOTES
oxymesh firmware built from \`$commit\` ($date).

| Board | Role | File | Partition table |
|---|---|---|---|
| ThinkNode M5 | companion (Bluetooth and USB) | \`oxymesh-m5.elf\` | \`partitions.csv\` |
| ThinkNode M5 | repeater | \`oxymesh-m5-repeater.elf\` | \`partitions.csv\` |
| ThinkNode M5 | room server | \`oxymesh-m5-room.elf\` | \`partitions.csv\` |
| Heltec V4 | companion | \`oxymesh-heltec-v4.elf\` | \`partitions-heltec-v4.csv\` |
| Heltec V4 | repeater | \`oxymesh-heltec-v4-repeater.elf\` | \`partitions-heltec-v4.csv\` |
| Heltec V4 | room server | \`oxymesh-heltec-v4-room.elf\` | \`partitions-heltec-v4.csv\` |

**Flash** with [espflash](https://github.com/esp-rs/espflash) (\`cargo install espflash --locked\`), the file and its partition table in the same folder:

\`\`\`sh
espflash flash --chip esp32s3 --flash-size 4mb --partition-table partitions.csv oxymesh-m5.elf --port <port>
espflash flash --chip esp32s3 --flash-size 16mb --partition-table partitions-heltec-v4.csv oxymesh-heltec-v4.elf --port <port>
\`\`\`

- Flashing writes the bootloader, the partition table and the firmware only. oxymesh's own data (\`mcdata\`) and the MeshCore C++ firmware's (its SPIFFS and NVS) are left as they are (ADR-0006).
- A board that ran the C++ firmware imports its identity, settings, contacts and channels at first boot. oxymesh never writes the C++ firmware's storage, so flashing the C++ release back finds it as it was.
- Check a download with \`shasum -a 256 -c SHA256SUMS\`.
- Debug information is stripped; for a \`debug-log\` build or backtraces, build from source (\`scripts/flash.sh\`, README).

You are responsible for using the radio legally where you are (README, "Radio use").
NOTES
echo "release files in $dist:"
ls -l "$dist"
