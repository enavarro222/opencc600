#!/bin/sh
# Builds the files of the in-browser demo (docs/demo/): the firmware as a clean flash image (the IDF
# bootloader espflash ships, tools/demo/partitions.csv, our app; storage blank, nothing from a device),
# its ELF without debug info (symbols for the emulator's virtual BLE controller), the S3 mask ROM and
# the WebAssembly emulator from an esp32sim checkout (github.com/enavarro222/esp32sim, with the CC600
# board; `tools/wasm-build.sh` and `tools/fetch-rom-elfs.sh web/wasm/fw` run there first).
#
# usage: ESP32SIM=~/workspace_perso/esp32sim tools/demo/build.sh
set -eu
cd "$(dirname "$0")/../.."
: "${ESP32SIM:?ESP32SIM: path to the esp32sim checkout}"
OUT=docs/demo
ELF=firmware/target/xtensa-esp32s3-none-elf/release/opencc600
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

unset BINDGEN_EXTRA_CLANG_ARGS   # host-test setting: breaks the littlefs bindings of the firmware
(cd firmware && . ~/export-esp.sh >/dev/null && cargo build --release)

mkdir -p "$OUT/fw" "$OUT/sim/wasm"
espflash save-image --chip esp32s3 --flash-size 16mb --merge --partition-table tools/demo/partitions.csv \
  "$ELF" "$tmp/merged.bin" >/dev/null
# the merged image is padded with 0xff to 16 MB: the emulator's flash starts erased anyway
python3 - "$tmp/merged.bin" "$OUT/fw/cc600-flash.bin" <<'EOF'
import sys
d = open(sys.argv[1], 'rb').read().rstrip(b'\xff')
n = -(-len(d) // 4096) * 4096
open(sys.argv[2], 'wb').write(d + b'\xff' * (n - len(d)))
EOF
STRIP=$(ls ~/.rustup/toolchains/esp/xtensa-esp-elf/*/xtensa-esp-elf/bin/xtensa-esp32s3-elf-strip | head -1)
"$STRIP" --strip-debug -o "$OUT/fw/cc600.elf" "$ELF"

cp "$ESP32SIM/web/wasm/fw/esp32s3_rev0_rom.elf" "$OUT/fw/"
for f in worker.js jit.mjs pacing.mjs experiments.mjs esp32sim.wasm; do cp "$ESP32SIM/web/wasm/$f" "$OUT/sim/wasm/"; done
cp "$ESP32SIM/LICENSE" "$OUT/sim/LICENSE"
rev=$(git -C "$ESP32SIM" rev-parse HEAD)
cat > "$OUT/sim/README.md" <<EOF
# esp32sim, built for this demo

The WebAssembly ESP32-S3 emulator and its worker, from [esp32sim](https://github.com/mikroverk/esp32sim)
(MIT, \`LICENSE\`), built from the fork [enavarro222/esp32sim](https://github.com/enavarro222/esp32sim) at
commit \`$rev\`: the GEOID CC600 board (ST7789 on LCD_CAM, the nRF52 on UART2) and the virtual BLE
controller for esp-radio firmware.

\`../fw/esp32s3_rev0_rom.elf\` is Espressif's ESP32-S3 mask ROM
([esp-rom-elfs](https://github.com/espressif/esp-rom-elfs), Apache-2.0).
Regenerate everything with \`tools/demo/build.sh\`.
EOF
ls -la "$OUT/fw" "$OUT/sim/wasm"
