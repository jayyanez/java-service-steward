// SPDX-License-Identifier: Apache-2.0 OR MIT
package io.github.jayyanez.jss.bridge;

import java.lang.reflect.Method;
import java.util.Properties;
import java.util.concurrent.CountDownLatch;

/**
 * Control API for applications running under Java Service Steward.
 *
 * <p>Applications launched through {@link SimpleApp}, {@link StartStopApp} or
 * {@link JarApp} may call the static methods of this class at any time.
 * Applications that prefer explicit lifecycle callbacks call
 * {@link #start(ServiceListener, String[])} from their own {@code main}.
 * Every method is safe to call when the JVM was not launched by the wrapper;
 * see the individual descriptions.</p>
 */
public final class Steward {
    public static final int LOG_DEBUG = 1;
    public static final int LOG_INFO = 2;
    public static final int LOG_STATUS = 3;
    public static final int LOG_WARN = 4;
    public static final int LOG_ERROR = 5;
    public static final int LOG_FATAL = 6;
    public static final int LOG_ADVICE = 7;
    public static final int LOG_NOTICE = 8;

    /** Control event delivered when the Windows service is paused. */
    public static final int CONTROL_PAUSE = 1;
    /** Control event delivered when the Windows service is resumed. */
    public static final int CONTROL_RESUME = 2;

    private static volatile BackendClient backend;

    private Steward() {
    }

    static void attach(BackendClient client) {
        backend = client;
    }

    /** Returns {@code true} when this JVM is connected to the wrapper. */
    public static boolean isManaged() {
        return backend != null;
    }

    /** Returns the wrapper-assigned JVM invocation number, or 0. */
    public static int getJvmId() {
        return integerProperty("wrapper.jvmid", 0);
    }

    /** Returns the wrapper process identifier, or 0. */
    public static int getWrapperPid() {
        return integerProperty("wrapper.pid", 0);
    }

    /** Returns this JVM's process identifier, or 0 when it cannot be determined. */
    public static int getJavaPid() {
        // ProcessHandle exists on Java 9+; reflection keeps Java 8 bytecode
        // compatibility. The management fallback is reflective too so that a
        // runtime built without java.management can still start.
        try {
            Class<?> processHandle = Class.forName("java.lang.ProcessHandle");
            Method current = processHandle.getMethod("current");
            Object handle = current.invoke(null);
            Method pid = processHandle.getMethod("pid");
            long value = ((Long) pid.invoke(handle)).longValue();
            if (value > 0 && value <= Integer.MAX_VALUE) {
                return (int) value;
            }
        } catch (Exception ignored) {
            // Java 8 or a runtime without ProcessHandle.
        } catch (LinkageError ignored) {
            // A reduced runtime may omit implementation-specific dependencies.
        }

        try {
            Class<?> managementFactory = Class.forName("java.lang.management.ManagementFactory");
            Method runtimeMxBean = managementFactory.getMethod("getRuntimeMXBean");
            Object bean = runtimeMxBean.invoke(null);
            Class<?> runtimeMxBeanInterface = Class.forName("java.lang.management.RuntimeMXBean");
            Method getName = runtimeMxBeanInterface.getMethod("getName");
            return parseRuntimeName((String) getName.invoke(bean));
        } catch (Exception ignored) {
            return 0;
        } catch (LinkageError ignored) {
            return 0;
        }
    }

    private static int parseRuntimeName(String runtimeName) {
        int separator = runtimeName.indexOf('@');
        String candidate = separator < 0 ? runtimeName : runtimeName.substring(0, separator);
        try {
            return Integer.parseInt(candidate);
        } catch (NumberFormatException error) {
            return 0;
        }
    }

    /** Returns {@code true} when the wrapper runs as a Windows service. */
    public static boolean isLaunchedAsService() {
        return Boolean.parseBoolean(System.getProperty("wrapper.service", "false"));
    }

    /** Returns the wrapper version, or the bridge JAR version when not managed. */
    public static String getVersion() {
        String version = System.getProperty("jss.version", "");
        if (version.length() > 0) {
            return version;
        }
        Package current = Steward.class.getPackage();
        String implementation = current == null ? null : current.getImplementationVersion();
        return implementation == null ? "unknown" : implementation;
    }

    /** Returns the wrapper's active log file path, or an empty string. */
    public static String getLogFile() {
        BackendClient current = backend;
        return current == null ? "" : current.getWrapperLogFile();
    }

    /** Returns a copy of the wrapper's configuration properties (empty when not managed). */
    public static Properties getProperties() {
        BackendClient current = backend;
        return current == null ? new Properties() : current.copyProperties();
    }

    /** Writes a message through the wrapper's log; prints to stdout when not managed. */
    public static void log(int level, String message) {
        BackendClient current = backend;
        if (current == null) {
            System.out.println(message);
            return;
        }
        current.log(level, message);
    }

    /** Sends an advisory startup wait hint; ignored when not managed. */
    public static void signalStarting(int waitHintMillis) {
        BackendClient current = backend;
        if (current != null) {
            current.signalStarting(waitHintMillis);
        }
    }

    /**
     * Asks the wrapper to stop the JVM with the given exit code. The wrapper
     * answers with a stop request that runs the normal shutdown path. When
     * not managed, the JVM exits immediately with that code.
     */
    public static void stop(int exitCode) {
        BackendClient current = backend;
        if (current == null) {
            System.exit(exitCode);
            return;
        }
        current.requestStop(exitCode);
    }

    /** Asks the wrapper to restart the JVM; exits with code 0 when not managed. */
    public static void restart() {
        BackendClient current = backend;
        if (current == null) {
            System.exit(0);
            return;
        }
        current.requestRestart();
    }

    /**
     * Runs an application through lifecycle callbacks.
     *
     * <p>When the JVM was launched by the wrapper this method connects to the
     * control channel, waits for the start request, invokes
     * {@link ServiceListener#start(String[])} on the calling thread and then
     * returns; the application is expected to keep at least one non-daemon
     * thread alive while it runs. A stop request from the wrapper invokes
     * {@link ServiceListener#stop(int)} and exits the JVM with the returned
     * code. When the JVM was started directly (for example from an IDE), the
     * listener's {@code start} is simply invoked on the calling thread.</p>
     */
    public static void start(final ServiceListener listener, final String[] arguments) {
        if (listener == null) {
            throw new IllegalArgumentException("listener must not be null");
        }
        final String[] safeArguments = arguments == null ? new String[0] : arguments;
        if (!BackendClient.isManagedLaunch()) {
            Integer exitCode = listener.start(safeArguments);
            if (exitCode != null) {
                System.exit(exitCode.intValue());
            }
            return;
        }

        Launchers.printBanner();
        final BackendClient client = Launchers.connectOrExit();
        final CountDownLatch startRequested = new CountDownLatch(1);
        client.startControlThread(new BackendClient.Handler() {
            @Override
            public void onStart() {
                startRequested.countDown();
            }

            @Override
            public void onStop(int exitCode) {
                int code;
                try {
                    code = listener.stop(exitCode);
                } catch (RuntimeException error) {
                    error.printStackTrace(System.err);
                    code = exitCode == 0 ? 1 : exitCode;
                }
                client.reportStopped(code);
                System.exit(code);
            }

            @Override
            public void onPause() {
                listener.controlEvent(CONTROL_PAUSE);
            }

            @Override
            public void onResume() {
                listener.controlEvent(CONTROL_RESUME);
            }

            @Override
            public void onControl(int code) {
                listener.controlEvent(code);
            }
        });

        try {
            startRequested.await();
        } catch (InterruptedException interrupted) {
            Thread.currentThread().interrupt();
            return;
        }
        Integer exitCode;
        try {
            exitCode = listener.start(safeArguments);
        } catch (RuntimeException error) {
            client.markAbnormalExit();
            error.printStackTrace(System.err);
            System.exit(1);
            return;
        }
        if (exitCode != null) {
            client.requestStop(exitCode.intValue());
            return;
        }
        client.reportStarted();
    }

    private static int integerProperty(String name, int defaultValue) {
        try {
            return Integer.parseInt(System.getProperty(name, Integer.toString(defaultValue)));
        } catch (NumberFormatException error) {
            return defaultValue;
        }
    }
}
