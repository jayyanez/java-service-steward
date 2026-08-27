// SPDX-License-Identifier: Apache-2.0 OR MIT
import io.github.jayyanez.jss.bridge.Steward;

/** Long-running synthetic application driven by its first argument. */
public final class SyntheticApplication {
    private SyntheticApplication() {
    }

    public static void main(String[] arguments) throws Exception {
        System.out.println("JSS_SYNTHETIC_READY arguments=" + arguments.length);
        if (arguments.length > 0 && "restart".equals(arguments[0])) {
            System.out.println("JSS_SYNTHETIC_RESTART");
        }
        if (arguments.length > 0 && "encoding".equals(arguments[0])) {
            System.out.println("JSS_SYNTHETIC_JAVA_VERSION " + System.getProperty("java.version"));
            System.out.println("JSS_SYNTHETIC_FILE_ENCODING " + System.getProperty("file.encoding"));
            System.out.println("JSS_SYNTHETIC_NATIVE_ENCODING " + System.getProperty("native.encoding"));
            System.out.println("JSS_SYNTHETIC_STDOUT_ENCODING " + System.getProperty("stdout.encoding"));
            System.out.println("JSS_SYNTHETIC_ENCODING áéíóú €");
            System.out.println("JSS_SYNTHETIC_RESTART");
        }
        if (arguments.length > 0 && "stop".equals(arguments[0])) {
            Thread.sleep(15_000L);
            Steward.stop(0);
        }
        if (arguments.length > 0 && "hang-shutdown".equals(arguments[0])) {
            Runtime.getRuntime().addShutdownHook(new Thread(new Runnable() {
                @Override
                public void run() {
                    System.out.println("JSS_SYNTHETIC_SHUTDOWN_HOOK_BLOCKED");
                    try {
                        Thread.sleep(30_000L);
                    } catch (InterruptedException ignored) {
                        Thread.currentThread().interrupt();
                    }
                }
            }, "synthetic-blocked-shutdown-hook"));
        }
        while (true) {
            Thread.sleep(1_000L);
        }
    }
}
