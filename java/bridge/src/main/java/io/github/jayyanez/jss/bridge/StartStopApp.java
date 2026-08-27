// SPDX-License-Identifier: Apache-2.0 OR MIT
package io.github.jayyanez.jss.bridge;

import java.lang.reflect.Method;
import java.util.Arrays;
import java.util.concurrent.CountDownLatch;

/**
 * Launcher for applications that expose separate start and stop classes.
 *
 * <p>Parameters:
 * {@code <startClass> <startArgCount> <startArgs...> <stopClass> <waitForStopThreads> <stopArgCount> <stopArgs...>}.
 * {@code waitForStopThreads} is {@code true} or {@code false}; when true, the
 * bridge waits after invoking the stop class until no non-daemon application
 * threads remain before letting the JVM exit.</p>
 */
public final class StartStopApp {
    private StartStopApp() {
    }

    public static void main(String[] arguments) {
        Launchers.printBanner();
        final Parameters parameters;
        try {
            parameters = Parameters.parse(arguments);
        } catch (IllegalArgumentException error) {
            System.err.println("StartStopApp: " + error.getMessage());
            System.err.println("Usage: <startClass> <startArgCount> <startArgs...> "
                    + "<stopClass> <waitForStopThreads> <stopArgCount> <stopArgs...>");
            System.exit(1);
            return;
        }
        final ClassLoader loader = Thread.currentThread().getContextClassLoader();
        final Method startMain;
        final Method stopMain;
        try {
            startMain = Launchers.resolveMain(parameters.startClass, loader);
            stopMain = Launchers.resolveMain(parameters.stopClass, loader);
        } catch (Exception error) {
            System.err.println("Unable to load the application classes: " + error);
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
                            Launchers.invokeMain(startMain, parameters.startArguments, backend);
                        }
                    }, loader);
                }
                backend.reportStarted();
                applicationStarted.countDown();
            }

            @Override
            public void onStop(final int exitCode) {
                Thread stopThread = new Thread(new Runnable() {
                    @Override
                    public void run() {
                        Launchers.invokeMain(stopMain, parameters.stopArguments, backend);
                    }
                }, Launchers.STOP_THREAD_NAME);
                stopThread.setContextClassLoader(loader);
                stopThread.setDaemon(false);
                stopThread.start();
                try {
                    stopThread.join();
                } catch (InterruptedException interrupted) {
                    Thread.currentThread().interrupt();
                }
                if (parameters.waitForStopThreads) {
                    Launchers.waitForApplicationThreads();
                }
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

    static final class Parameters {
        final String startClass;
        final String[] startArguments;
        final String stopClass;
        final boolean waitForStopThreads;
        final String[] stopArguments;

        private Parameters(String startClass, String[] startArguments, String stopClass,
                boolean waitForStopThreads, String[] stopArguments) {
            this.startClass = startClass;
            this.startArguments = startArguments;
            this.stopClass = stopClass;
            this.waitForStopThreads = waitForStopThreads;
            this.stopArguments = stopArguments;
        }

        static Parameters parse(String[] arguments) {
            int cursor = 0;
            if (arguments.length < 2) {
                throw new IllegalArgumentException("missing start class and start argument count");
            }
            String startClass = arguments[cursor++];
            int startCount = Launchers.parseCount(arguments[cursor++], "start argument count");
            if (arguments.length < cursor + startCount) {
                throw new IllegalArgumentException("fewer start arguments than declared");
            }
            String[] startArguments = Arrays.copyOfRange(arguments, cursor, cursor + startCount);
            cursor += startCount;
            if (arguments.length < cursor + 3) {
                throw new IllegalArgumentException(
                        "missing stop class, waitForStopThreads flag or stop argument count");
            }
            String stopClass = arguments[cursor++];
            String waitFlag = arguments[cursor++].trim();
            boolean waitForStopThreads;
            if ("true".equalsIgnoreCase(waitFlag)) {
                waitForStopThreads = true;
            } else if ("false".equalsIgnoreCase(waitFlag)) {
                waitForStopThreads = false;
            } else {
                throw new IllegalArgumentException("waitForStopThreads must be true or false, found: " + waitFlag);
            }
            int stopCount = Launchers.parseCount(arguments[cursor++], "stop argument count");
            if (arguments.length < cursor + stopCount) {
                throw new IllegalArgumentException("fewer stop arguments than declared");
            }
            String[] stopArguments = Arrays.copyOfRange(arguments, cursor, cursor + stopCount);
            return new Parameters(startClass, startArguments, stopClass, waitForStopThreads, stopArguments);
        }
    }
}
