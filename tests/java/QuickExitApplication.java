// SPDX-License-Identifier: Apache-2.0 OR MIT
/** Returns from main immediately; the JVM must then exit on its own. */
public final class QuickExitApplication {
    private QuickExitApplication() {
    }

    public static void main(String[] arguments) {
        System.out.println("JSS_QUICK_EXIT_RETURNING");
    }
}
