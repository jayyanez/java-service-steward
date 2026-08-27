// SPDX-License-Identifier: Apache-2.0 OR MIT
package io.github.jayyanez.jss.bridge;

import java.lang.reflect.Method;
import java.util.Arrays;
import java.util.concurrent.CountDownLatch;

/**
 * Launcher for applications that expose an ordinary {@code main(String[])}.
 *
 * <p>Parameters: {@code <application main class> [application arguments...]}.
 * The wrapper selects this class when {@code wrapper.java.mainclass} names the
 * simple-application launcher alias, so existing configuration files keep
 * their value.</p>
 *
 * <p>The application runs on a non-daemon thread. When it returns and no other
 * non-daemon threads remain, the JVM exits and the wrapper applies its
 * {@code wrapper.on_exit.*} policy. When the wrapper asks the JVM to stop, the
 * JVM's shutdown hooks run and the application is expected to stop from
 * there.</p>
 */
public final class SimpleApp {
    private SimpleApp() {
    }

    public static void main(String[] arguments) {
        Launchers.printBanner();
        if (arguments.length == 0 || arguments[0].trim().length() == 0) {
            System.err.println("SimpleApp requires the application main class as its first argument.");
            System.exit(1);
            return;
        }
        final String mainClassName = arguments[0];
        final String[] applicationArguments = Arrays.copyOfRange(arguments, 1, arguments.length);
        final ClassLoader loader = Thread.currentThread().getContextClassLoader();
        final Method main;
        try {
            main = Launchers.resolveMain(mainClassName, loader);
        } catch (Exception error) {
            System.err.println("Unable to load the application main class " + mainClassName
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
}
