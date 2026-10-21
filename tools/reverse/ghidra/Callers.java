// Prints the instructions just before each call to the given function (the argument setup). Args: addr
import ghidra.app.script.GhidraScript;
import ghidra.program.model.listing.*;
import ghidra.program.model.symbol.*;
public class Callers extends GhidraScript {
    @Override protected void run() throws Exception {
        var target = toAddr(Long.decode(getScriptArgs()[0]));
        for (Reference r : getReferencesTo(target)) {
            Instruction ins = getInstructionAt(r.getFromAddress());
            println("call from " + r.getFromAddress() + " in " + getFunctionContaining(r.getFromAddress()));
            Instruction p = ins;
            for (int i = 0; i < 4 && p != null; i++) p = p.getPrevious();
            for (int i = 0; i < 5 && p != null; i++, p = p.getNext()) println("   " + p.getAddress() + " " + p);
        }
    }
}
