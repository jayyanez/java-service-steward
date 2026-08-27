// SPDX-License-Identifier: Apache-2.0 OR MIT
/** Main-Class of the executable JAR used by the JarApp launcher test. */
public final class JarMainApplication {
    private JarMainApplication() {
    }

    public static void main(String[] arguments) throws Exception {
        System.out.println("JSS_JAR_READY arguments=" + arguments.length);
        while (true) {
            Thread.sleep(1_000L);
        }
    }
}
