// Names the ESP32-S3 mask ROM functions (memcpy, printf, the libgcc soft float...) from the ROM linker scripts
// (`name = 0x4000....;`), on an uninitialized block over the ROM, so the calls into it read as names.
// Headless pre-script; arguments: the .ld files (esp-rom-sys ld/esp32s3/rom/*.ld).
// @category opencc600

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.mem.Memory;
import ghidra.program.model.symbol.SourceType;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

public class AddRomSymbols extends GhidraScript {
    private static final long ROM_START = 0x40000000L;
    private static final long ROM_SIZE = 0x60000L;

    @Override
    protected void run() throws Exception {
        Memory mem = currentProgram.getMemory();
        Address start = toAddr(ROM_START);
        if (mem.getBlock(start) == null) {
            mem.createUninitializedBlock("rom", start, ROM_SIZE, false).setExecute(true);
        }
        Pattern p = Pattern.compile("(?:PROVIDE\\s*\\(\\s*)?([A-Za-z_][A-Za-z0-9_]*)\\s*=\\s*(0x[0-9a-fA-F]+)");
        int n = 0;
        for (String file : getScriptArgs()) {
            for (String line : Files.readAllLines(Path.of(file))) {
                Matcher m = p.matcher(line);
                if (!m.find()) {
                    continue;
                }
                long addr = Long.decode(m.group(2));
                if (addr < ROM_START || addr >= ROM_START + ROM_SIZE) {
                    continue;
                }
                Address a = toAddr(addr);
                createLabel(a, m.group(1), true, SourceType.IMPORTED);
                if (getFunctionAt(a) == null) {
                    createFunction(a, m.group(1));
                }
                n++;
            }
        }
        println("ROM symbols: " + n);
    }
}
