// Decompiles every function that uses one of the given strings, through the Xtensa literal pools (the code loads
// the pool entry with `l32r`, the entry holds the string address), plus the functions they call that have no
// name yet, one level down. Headless post-script; arguments: OUT.c then the strings, or `0x` addresses of data
// (a constant in a literal pool...).
// @category opencc600

import ghidra.app.decompiler.DecompInterface;
import ghidra.app.decompiler.DecompileResults;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import ghidra.program.model.symbol.Reference;

import java.io.PrintWriter;
import java.nio.charset.StandardCharsets;
import java.util.LinkedHashSet;
import java.util.Set;

public class DecompileRefs extends GhidraScript {
    @Override
    protected void run() throws Exception {
        String[] args = getScriptArgs();
        Set<Function> functions = new LinkedHashSet<>();
        for (int i = 1; i < args.length; i++) {
            if (args[i].startsWith("0x")) {
                Address target = toAddr(Long.decode(args[i]));
                for (Function f : users(target)) {
                    println(args[i] + " used by " + f.getName() + " @ " + f.getEntryPoint());
                    functions.add(f);
                }
                continue;
            }
            byte[] needle = args[i].getBytes(StandardCharsets.US_ASCII);
            Address found = find(currentProgram.getMinAddress(), needle);
            while (found != null) {
                for (Function f : users(found)) {
                    println(args[i] + " @ " + found + " used by " + f.getName() + " @ " + f.getEntryPoint());
                    functions.add(f);
                }
                found = find(found.add(1), needle);
            }
        }
        Set<Function> all = new LinkedHashSet<>(functions);
        for (Function f : functions) {
            for (Function callee : f.getCalledFunctions(monitor)) {
                if (callee.getName().startsWith("FUN_")) {
                    all.add(callee);
                }
            }
        }
        DecompInterface dec = new DecompInterface();
        dec.openProgram(currentProgram);
        try (PrintWriter out = new PrintWriter(args[0], StandardCharsets.UTF_8)) {
            for (Function f : all) {
                DecompileResults r = dec.decompileFunction(f, 120, monitor);
                out.println("// ---- " + f.getName() + " @ " + f.getEntryPoint()
                        + (functions.contains(f) ? " (uses the strings)" : " (callee)"));
                out.println(r.decompileCompleted() ? r.getDecompiledFunction().getC() : "// " + r.getErrorMessage());
            }
        }
        println("decompiled " + all.size() + " functions into " + args[0]);
    }

    /** Functions using the data at `target`: directly, or through a literal pool entry pointing to it. */
    private Set<Function> users(Address target) {
        Set<Function> out = new LinkedHashSet<>();
        for (Reference r : getReferencesTo(target)) {
            Function f = getFunctionContaining(r.getFromAddress());
            if (f != null) {
                out.add(f);
            }
            for (Reference r2 : getReferencesTo(r.getFromAddress())) {
                Function g = getFunctionContaining(r2.getFromAddress());
                if (g != null) {
                    out.add(g);
                }
            }
        }
        return out;
    }
}
