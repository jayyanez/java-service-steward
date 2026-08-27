// SPDX-License-Identifier: Apache-2.0 OR MIT
package io.github.jayyanez.jss.bridge;

import java.io.File;
import java.lang.reflect.Method;
import java.net.URL;
import java.net.URLClassLoader;
import java.util.Arrays;
import java.util.concurrent.CountDownLatch;
import java.util.jar.Attributes;
import java.util.jar.JarFile;
import java.util.jar.Manifest;

/**
 * Launcher for executable JAR files.
 *
 * <p>Parameters: {@code <jar path> [application arguments...]}. The
 * {@code Main-Class} manifest attribute names the application entry point.
 * The JAR is loaded through a dedicated class loader whose parent is the
 * system class loader, so the JAR does not need to be on
 * {@code wrapper.java.classpath}.</p>
 */
public final class JarApp {
    private JarApp() {
    }

    public static void main(String[] arguments) {
        Launchers.printBanner();
        if (arguments.length == 0 || arguments[0].trim().length() == 0) {
            System.err.println("JarApp requires the JAR path as its first argument.");
            System.exit(1);
            return;
        }
        final File jar = new File(arguments[0]);
        final String[] applicationArguments = Arrays.copyOfRange(arguments, 1, arguments.length);
        final String mainClassName;
        final URLClassLoader loader;
        try {
            mainClassName = readMainClass(jar);
            loader = new URLClassLoader(new URL[] { jar.toURI().toURL() },
                    ClassLoader.getSystemClassLoader());
        } catch (Exception error) {
            System.err.println("Unable to open " + jar + ": " + error.getMessage());
            System.exit(1);
            return;
        }
        final Method main;
        try {
            main = Launchers.resolveMain(mainClassName, loader);
        } catch (Exception error) {
            System.err.println("Unable to load Main-Class " + mainClassName + " from " + jar
                    + ": " + error);
            System.exit(1);
            return;
        }

        final BackendClient backend = Launchers.connectOrExit();
        final Thread[] applicationThread = new Thread[1];
        final CountDownLatch applicationStarted = new CountDownLatch(1);
        backend.startControlThread(new BackendClient.Handler() {
            @Override
            public void onStart() {
                synchronized (applicationThread) {
                    if (applicationThread[0] != null) {
                        return;
                    }
                    applicationThread[0] = Launchers.startApplicationThread(new Runnable() {
                        @Override
                        public void run() {
                            Launchers.invokeMain(main, applicationArguments, backend);
                        }
                    }, loader);
                }
                backend.reportStarted();
                applicationStarted.countDown();
            }

            @Override
            public void onStop(int exitCode) {
                backend.reportStopped(exitCode);
                System.exit(exitCode);
            }

            @Override
            public void onPause() {
            }

            @Override
            public void onResume() {
            }

            @Override
            public void onControl(int code) {
            }
        });
        // Keep a non-daemon thread alive until the application thread exists;
        // afterwards the application itself keeps the JVM alive.
        Launchers.awaitUninterruptibly(applicationStarted);
    }

    private static String readMainClass(File jar) throws Exception {
        if (!jar.isFile()) {
            throw new IllegalArgumentException("not a file");
        }
        JarFile jarFile = new JarFile(jar);
        try {
            Manifest manifest = jarFile.getManifest();
            String mainClass = manifest == null
                    ? null
                    : manifest.getMainAttributes().getValue(Attributes.Name.MAIN_CLASS);
            if (mainClass == null || mainClass.trim().length() == 0) {
                throw new IllegalArgumentException("the JAR manifest has no Main-Class attribute");
            }
            return mainClass.trim();
        } finally {
            jarFile.close();
        }
    }
}
