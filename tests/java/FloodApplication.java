// SPDX-License-Identifier: Apache-2.0 OR MIT
import java.io.PrintStream;

import io.github.jayyanez.jss.bridge.Steward;

/** Emits many output lines as fast as possible, then asks the wrapper to stop. */
public final class FloodApplication {
    private FloodApplication() {
    }

    public static void main(String[] arguments) throws Exception {
        int lines = arguments.length == 0 ? 50_000 : Integer.parseInt(arguments[0]);
        PrintStream out = System.out;
        for (int index = 1; index <= lines; index++) {
            out.println("JSS_FLOOD_LINE " + index);
        }
        out.println("JSS_FLOOD_DONE " + lines);
        out.flush();
        Steward.stop(0);
        while (true) {
            Thread.sleep(1_000L);
        }
    }
}
