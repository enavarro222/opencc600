// Decompiles the functions at the given addresses and their callees down to DEPTH. Args: OUT.c DEPTH addr...
import ghidra.app.decompiler.*;
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.Function;
import java.io.PrintWriter;
import java.util.*;
public class DecompileAt extends GhidraScript {
    @Override protected void run() throws Exception {
        String[] a = getScriptArgs();
        int depth = Integer.parseInt(a[1]);
        LinkedHashSet<Function> seen = new LinkedHashSet<>();
        ArrayDeque<Object[]> q = new ArrayDeque<>();
        for (int i = 2; i < a.length; i++) {
            var addr = toAddr(Long.decode(a[i]));
            Function f = getFunctionAt(addr);
            if (f == null) { disassemble(addr); f = createFunction(addr, null); }
            if (f == null) f = getFunctionContaining(addr);
            if (f != null) q.add(new Object[]{f, 0});
        }
        while (!q.isEmpty()) {
            Object[] e = q.poll(); Function f = (Function) e[0]; int d = (int) e[1];
            if (!seen.add(f)) continue;
            if (d < depth) for (Function c : f.getCalledFunctions(monitor)) q.add(new Object[]{c, d + 1});
        }
        DecompInterface di = new DecompInterface(); di.openProgram(currentProgram);
        try (PrintWriter out = new PrintWriter(a[0])) {
            for (Function f : seen) {
                DecompileResults r = di.decompileFunction(f, 60, monitor);
                out.println("// ---- " + f.getName() + " @ " + f.getEntryPoint());
                out.println(r.getDecompiledFunction() == null ? "// failed" : r.getDecompiledFunction().getC());
            }
        }
    }
}
