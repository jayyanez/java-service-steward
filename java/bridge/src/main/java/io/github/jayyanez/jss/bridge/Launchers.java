// SPDX-License-Identifier: Apache-2.0 OR MIT
package io.github.jayyanez.jss.bridge;

import java.lang.reflect.InvocationTargetException;
import java.lang.reflect.Method;
import java.lang.reflect.Modifier;
import java.util.Map;
import java.util.concurrent.CountDownLatch;

/** Shared plumbing for the launcher classes. */
final class Launchers {
    static final String APP_THREAD_NAME = "jss-app-main";
    static final String STOP_THREAD_NAME = "jss-app-stop";
    private static final long STOP_THREAD_POLL_MILLIS = 100L;

    private Launchers() {
    }

    static void printBanner() {
        // The Rust executable captures this line into wrapper.log; it doubles
        // as a version marker for support requests.
        System.out.println("Java Service Steward bridge " + Steward.getVersion() + " initializing");
    }

    /** Connects to the wrapper and returns the client, or terminates the JVM. */
    static BackendClient connectOrExit() {
        try {
            BackendClient backend = BackendClient.fromSystemProperties();
            backend.connect();
            Steward.attach(backend);
            if (!Boolean.parseBoolean(System.getProperty("wrapper.disable_shutdown_hook", "false"))) {
                installShutdownHook(backend);
            }
            return backend;
        } catch (Exception error) {
            System.err.println("Unable to connect to the Java Service Steward control channel: "
                    + error.getMessage());
            System.exit(1);
            throw new IllegalStateException("unreachable");
        }
    }

    private static void installShutdownHook(final BackendClient backend) {
        Runtime.getRuntime().addShutdownHook(new Thread(new Runnable() {
            @Override
            public void run() {
                backend.onJvmShutdown();
            }
        }, "jss-shutdown-hook"));
    }

    /** Resolves {@code public static void main(String[])} on the named class. */
    static Method resolveMain(String className, ClassLoader loader) throws Exception {
        Class<?> mainClass = Class.forName(className, true, loader);
        Method main = mainClass.getMethod("main", String[].class);
        if (!Modifier.isStatic(main.getModifiers()) || main.getReturnType() != Void.TYPE) {
            throw new NoSuchMethodException(className + ".main(String[]) must be public static void");
        }
        return main;
    }

    /**
     * Invokes the application {@code main} method. An exception thrown by the
     * application is printed and terminates the JVM with exit code 1.
     */
    static void invokeMain(Method main, String[] arguments, BackendClient backend) {
        try {
            main.invoke(null, new Object[] { arguments });
        } catch (InvocationTargetException error) {
            backend.markAbnormalExit();
            Throwable cause = error.getCause() == null ? error : error.getCause();
            cause.printStackTrace(System.err);
            System.exit(1);
        } catch (Throwable error) {
            backend.markAbnormalExit();
            error.printStackTrace(System.err);
            System.exit(1);
        }
    }

    /** Starts a non-daemon application thread with the given body. */
    static Thread startApplicationThread(Runnable body, ClassLoader loader) {
        Thread thread = new Thread(body, APP_THREAD_NAME);
        thread.setContextClassLoader(loader);
        thread.setDaemon(false);
        thread.start();
        return thread;
    }

    /**
     * Blocks until every non-daemon thread other than the current one and the
     * JVM's own threads has finished.
     */
    static void waitForApplicationThreads() {
        while (true) {
            boolean busy = false;
            for (Map.Entry<Thread, StackTraceElement[]> entry : Thread.getAllStackTraces().entrySet()) {
                Thread thread = entry.getKey();
                if (thread == Thread.currentThread() || thread.isDaemon() || !thread.isAlive()) {
                    continue;
                }
                String name = thread.getName();
                if ("DestroyJavaVM".equals(name)) {
                    continue;
                }
                busy = true;
                break;
            }
            if (!busy) {
                return;
            }
            try {
                Thread.sleep(STOP_THREAD_POLL_MILLIS);
            } catch (InterruptedException interrupted) {
                Thread.currentThread().interrupt();
                return;
            }
        }
    }

    /** Blocks the calling thread until the latch is released. */
    static void awaitUninterruptibly(CountDownLatch latch) {
        boolean interrupted = false;
        while (true) {
            try {
                latch.await();
                break;
            } catch (InterruptedException error) {
                interrupted = true;
            }
        }
        if (interrupted) {
            Thread.currentThread().interrupt();
        }
    }

    static int parseCount(String value, String what) {
        try {
            int count = Integer.parseInt(value.trim());
            if (count < 0) {
                throw new NumberFormatException();
            }
            return count;
        } catch (NumberFormatException error) {
            throw new IllegalArgumentException(what + " must be a non-negative integer, found: " + value);
        }
    }
}
