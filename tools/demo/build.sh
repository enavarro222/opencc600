#!/bin/sh
# Builds one version of the in-browser demo (docs/demo/) into docs/demo/builds/NAME/: the firmware as a
# clean flash image (the IDF bootloader espflash ships, tools/demo/partitions.csv, our app; storage blank,
# nothing from a device), its ELF without debug info (symbols for the emulator's virtual BLE controller),
# the WebAssembly emulator and a build.json; the S3 mask ROM goes to docs/demo/rom/, then versions.json is
# rewritten from every build present.
#
# The emulator comes from the esp32sim fork with the CC600 board (github.com/enavarro222/esp32sim) at the
# commit in tools/demo/esp32sim.rev, cloned into target/demo/esp32sim, or from the checkout in $ESP32SIM.
#
# usage: [ESP32SIM=~/workspace_perso/esp32sim] tools/demo/build.sh [NAME]     (NAME: local by default)
set -eu
cd "$(dirname "$0")/../.."
NAME=${1:-local}
case $NAME in ''|.*|*[!A-Za-z0-9._-]*) echo "build name: letters, digits, '.', '_', '-' only" >&2; exit 2;; esac
OUT=docs/demo
DIR=$OUT/builds/$NAME
ELF=firmware/target/xtensa-esp32s3-none-elf/release/opencc600
REV=$(cat tools/demo/esp32sim.rev)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

if [ -z "${ESP32SIM:-}" ]; then
  ESP32SIM=target/demo/esp32sim
  [ -d "$ESP32SIM/.git" ] || git clone -q https://github.com/enavarro222/esp32sim "$ESP32SIM"
  git -C "$ESP32SIM" fetch -q origin "$REV" 2>/dev/null || git -C "$ESP32SIM" fetch -q origin
  git -C "$ESP32SIM" checkout -q "$REV"
elif [ "$(git -C "$ESP32SIM" rev-parse HEAD)" != "$REV" ]; then
  echo "note: $ESP32SIM is not at tools/demo/esp32sim.rev ($REV)" >&2
fi
(cd "$ESP32SIM" && tools/wasm-build.sh >/dev/null && tools/fetch-rom-elfs.sh web/wasm/fw >/dev/null)

unset BINDGEN_EXTRA_CLANG_ARGS   # host-test setting: breaks the littlefs bindings of the firmware
(cd firmware && . ~/export-esp.sh >/dev/null && cargo build --release)

rm -rf "$DIR"
mkdir -p "$DIR/fw" "$DIR/sim/wasm" "$OUT/rom"
espflash save-image --chip esp32s3 --flash-size 16mb --merge --partition-table tools/demo/partitions.csv \
  "$ELF" "$tmp/merged.bin" >/dev/null
# the merged image is padded with 0xff to 16 MB: the emulator's flash starts erased anyway
python3 - "$tmp/merged.bin" "$DIR/fw/cc600-flash.bin" <<'EOF'
import sys
d = open(sys.argv[1], 'rb').read().rstrip(b'\xff')
n = -(-len(d) // 4096) * 4096
open(sys.argv[2], 'wb').write(d + b'\xff' * (n - len(d)))
EOF
STRIP=$(ls ~/.rustup/toolchains/esp/xtensa-esp-elf/*/xtensa-esp-elf/bin/xtensa-esp32s3-elf-strip | head -1)
"$STRIP" --strip-debug -o "$DIR/fw/cc600.elf" "$ELF"

cp "$ESP32SIM/web/wasm/fw/esp32s3_rev0_rom.elf" "$OUT/rom/"
for f in worker.js jit.mjs pacing.mjs experiments.mjs esp32sim.wasm; do cp "$ESP32SIM/web/wasm/$f" "$DIR/sim/wasm/"; done
cp "$ESP32SIM/LICENSE" "$DIR/sim/LICENSE"
sim=$(git -C "$ESP32SIM" rev-parse HEAD)
cat > "$DIR/sim/README.md" <<EOF
# esp32sim, built for this demo

The WebAssembly ESP32-S3 emulator and its worker, from [esp32sim](https://github.com/mikroverk/esp32sim)
(MIT, \`LICENSE\`), built from the fork [enavarro222/esp32sim](https://github.com/enavarro222/esp32sim) at
commit \`$sim\`: the GEOID CC600 board (ST7789 on LCD_CAM, the nRF52 on UART2, the GNSS receiver on
UART0) and the virtual BLE controller for esp-radio firmware.

\`../../../rom/esp32s3_rev0_rom.elf\` is Espressif's ESP32-S3 mask ROM
([esp-rom-elfs](https://github.com/espressif/esp-rom-elfs), Apache-2.0).
Regenerate everything with \`tools/demo/build.sh\`.
EOF
python3 tools/demo/versions.py build-info "$DIR" "$NAME" "$(git rev-parse HEAD)" "$sim"
python3 tools/demo/versions.py index "$OUT"
ls -la "$DIR/fw" "$DIR/sim/wasm"
