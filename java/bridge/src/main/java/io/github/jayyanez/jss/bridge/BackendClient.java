// SPDX-License-Identifier: Apache-2.0 OR MIT
package io.github.jayyanez.jss.bridge;

import java.io.BufferedInputStream;
import java.io.ByteArrayOutputStream;
import java.io.EOFException;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.net.SocketTimeoutException;
import java.nio.charset.StandardCharsets;
import java.util.Properties;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Client side of the local control channel between the Java Service Steward
 * executable and this JVM.
 *
 * <p>The channel is a loopback TCP socket. Every packet is one type byte, a
 * UTF-8 payload and a NUL terminator. The control loop runs on a daemon thread
 * so that the JVM exits naturally once the application has no non-daemon
 * threads left.</p>
 */
final class BackendClient {
    /** Lifecycle callbacks invoked from the control thread. */
    interface Handler {
        void onStart();

        void onStop(int exitCode);

        void onPause();

        void onResume();

        void onControl(int code);
    }

    private static final int START = 100;
    private static final int STOP = 101;
    private static final int RESTART = 102;
    private static final int PING = 103;
    private static final int START_PENDING = 105;
    private static final int STARTED = 106;
    private static final int STOPPED = 107;
    private static final int KEY = 110;
    private static final int BAD_KEY = 111;
    private static final int LOW_LOG_LEVEL = 112;
    private static final int SERVICE_CONTROL = 114;
    private static final int PROPERTIES = 115;
    private static final int LOG_BASE = 116;
    private static final int LOGFILE = 134;
    private static final int PAUSE = 138;
    private static final int RESUME = 139;
    private static final int GC = 140;

    private static final int MAX_PACKET_SIZE = 1024 * 1024;
    private static final int CONNECT_TIMEOUT_MILLIS = 5_000;
    private static final int READ_TIMEOUT_MILLIS = 1_000;

    private final String key;
    private final int port;
    private final AtomicBoolean wrapperRequestedStop = new AtomicBoolean(false);
    private final AtomicBoolean abnormalExit = new AtomicBoolean(false);
    // Set by the shutdown hook: read failures after this point are the
    // hook closing the socket, not a lost channel.
    private final AtomicBoolean jvmShuttingDown = new AtomicBoolean(false);
    private final AtomicBoolean stoppedSent = new AtomicBoolean(false);
    private final Properties properties = new Properties();

    private volatile int lowLogLevel = 2;
    private volatile String wrapperLogFile = "";
    private volatile long lastWrapperPacketMillis = System.currentTimeMillis();

    private Socket socket;
    private InputStream input;
    private OutputStream output;

    // Partial packet state; survives socket read timeouts so that a packet
    // split across two reads is never misinterpreted.
    private int pendingCode = -1;
    private final ByteArrayOutputStream pendingMessage = new ByteArrayOutputStream(128);

    private BackendClient(String key, int port) {
        this.key = key;
        this.port = port;
    }

    /** Returns {@code true} when the JVM was launched by the wrapper. */
    static boolean isManagedLaunch() {
        return System.getProperty("wrapper.key", "").length() > 0;
    }

    static BackendClient fromSystemProperties() {
        String configuredKey = System.getProperty("wrapper.key", "");
        String configuredPort = System.getProperty("wrapper.port", "");
        if (configuredKey.length() == 0) {
            throw new IllegalArgumentException("missing system property wrapper.key");
        }
        try {
            int parsedPort = Integer.parseInt(configuredPort);
            if (parsedPort < 1 || parsedPort > 65_535) {
                throw new NumberFormatException("outside TCP port range");
            }
            return new BackendClient(configuredKey, parsedPort);
        } catch (NumberFormatException error) {
            throw new IllegalArgumentException("invalid system property wrapper.port", error);
        }
    }

    synchronized void connect() throws IOException {
        closeSocket();
        Socket candidate = new Socket();
        candidate.connect(new InetSocketAddress("127.0.0.1", port), CONNECT_TIMEOUT_MILLIS);
        candidate.setTcpNoDelay(true);
        candidate.setSoTimeout(READ_TIMEOUT_MILLIS);
        socket = candidate;
        input = new BufferedInputStream(candidate.getInputStream(), 8_192);
        output = candidate.getOutputStream();
        send(KEY, key);
    }

    /**
     * Starts the control loop on a daemon thread. Protocol failures terminate
     * the JVM with exit code 1 so that the wrapper can launch a replacement.
     */
    void startControlThread(final Handler handler) {
        Thread control = new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    controlLoop(handler);
                } catch (IOException error) {
                    if (wrapperRequestedStop.get() || jvmShuttingDown.get()) {
                        return;
                    }
                    abnormalExit.set(true);
                    System.err.println("Java Service Steward control channel failed: "
                            + error.getMessage());
                    System.exit(1);
                } catch (RuntimeException error) {
                    if (jvmShuttingDown.get()) {
                        return;
                    }
                    abnormalExit.set(true);
                    error.printStackTrace(System.err);
                    System.exit(1);
                }
            }
        }, "jss-control");
        control.setDaemon(true);
        control.start();
    }

    private void controlLoop(Handler handler) throws IOException {
        while (!wrapperRequestedStop.get()) {
            Packet packet;
            try {
                packet = readPacket();
            } catch (SocketTimeoutException error) {
                if (wrapperRequestedStop.get() || jvmShuttingDown.get()) {
                    return;
                }
                if (pingTimedOut()) {
                    abnormalExit.set(true);
                    throw new IOException("no packet from the wrapper for "
                            + pingTimeoutMillis() / 1_000L + " seconds", error);
                }
                continue;
            }
            lastWrapperPacketMillis = System.currentTimeMillis();
            handle(packet, handler);
        }
    }

    private void handle(Packet packet, Handler handler) throws IOException {
        switch (packet.code) {
            case START:
                handler.onStart();
                break;
            case STOP:
                wrapperRequestedStop.set(true);
                handler.onStop(parseInt(packet.message, 0));
                break;
            case PING:
                send(PING, packet.message);
                break;
            case LOW_LOG_LEVEL:
                lowLogLevel = parseInt(packet.message, 2);
                break;
            case LOGFILE:
                wrapperLogFile = packet.message;
                break;
            case PROPERTIES:
                loadProperties(packet.message);
                break;
            case PAUSE:
                handler.onPause();
                break;
            case RESUME:
                handler.onResume();
                break;
            case SERVICE_CONTROL:
                handler.onControl(parseInt(packet.message, 0));
                break;
            case GC:
                System.gc();
                break;
            case BAD_KEY:
                abnormalExit.set(true);
                throw new IOException("the wrapper rejected the control channel key");
            default:
                // Unknown packet types are ignored so that newer wrappers can
                // add advisory packets without breaking older bridges.
                break;
        }
    }

    /** Reports that the application is running. */
    void reportStarted() {
        sendQuietly(STARTED, "");
    }

    /** Reports that the application finished its orderly stop. */
    void reportStopped(int exitCode) {
        if (stoppedSent.compareAndSet(false, true)) {
            sendQuietly(STOPPED, Integer.toString(exitCode));
        }
    }

    void requestStop(int exitCode) {
        sendQuietly(STOP, Integer.toString(exitCode));
    }

    void requestRestart() {
        sendQuietly(RESTART, "restart");
    }

    void signalStarting(int waitHintMillis) {
        sendQuietly(START_PENDING, Integer.toString(Math.max(0, waitHintMillis)));
    }

    void log(int level, String message) {
        if (level < 1 || level > 8 || level < lowLogLevel) {
            return;
        }
        sendQuietly(LOG_BASE + level, message == null ? "null" : message);
    }

    Properties copyProperties() {
        Properties copy = new Properties();
        synchronized (properties) {
            copy.putAll(properties);
        }
        return copy;
    }

    String getWrapperLogFile() {
        return wrapperLogFile;
    }

    boolean wasStopRequestedByWrapper() {
        return wrapperRequestedStop.get();
    }

    void markAbnormalExit() {
        abnormalExit.set(true);
    }

    /**
     * Invoked from the JVM shutdown hook. When the JVM is going down on its
     * own (the application returned from {@code main} or called
     * {@code System.exit}), the wrapper is told that the application stopped.
     * The exit code of an arbitrary {@code System.exit} call cannot be
     * observed here; applications that need exit-code policy should call
     * {@link Steward#stop(int)} instead.
     */
    void onJvmShutdown() {
        jvmShuttingDown.set(true);
        if (!wrapperRequestedStop.get() && !abnormalExit.get()) {
            sendQuietly(STOP, "0");
        }
        closeSocketQuietly();
    }

    private Packet readPacket() throws IOException {
        InputStream currentInput;
        synchronized (this) {
            currentInput = input;
        }
        if (currentInput == null) {
            throw new EOFException("control socket is not connected");
        }
        while (true) {
            int value = currentInput.read();
            if (value < 0) {
                throw new EOFException("control socket closed");
            }
            if (pendingCode < 0) {
                pendingCode = value;
                continue;
            }
            if (value == 0) {
                Packet packet = new Packet(pendingCode,
                        new String(pendingMessage.toByteArray(), StandardCharsets.UTF_8));
                pendingCode = -1;
                pendingMessage.reset();
                return packet;
            }
            if (pendingMessage.size() >= MAX_PACKET_SIZE) {
                throw new IOException("control packet exceeds " + MAX_PACKET_SIZE + " bytes");
            }
            pendingMessage.write(value);
        }
    }

    private synchronized void send(int code, String message) throws IOException {
        if (output == null) {
            throw new EOFException("control socket is not connected");
        }
        byte[] bytes = message.getBytes(StandardCharsets.UTF_8);
        if (bytes.length > MAX_PACKET_SIZE) {
            throw new IOException("control packet exceeds " + MAX_PACKET_SIZE + " bytes");
        }
        byte[] frame = new byte[bytes.length + 2];
        frame[0] = (byte) code;
        System.arraycopy(bytes, 0, frame, 1, bytes.length);
        frame[frame.length - 1] = 0;
        output.write(frame);
        output.flush();
    }

    private void sendQuietly(int code, String message) {
        try {
            send(code, message);
        } catch (IOException ignored) {
            // The wrapper also observes the JVM process exit.
        }
    }

    private static int parseInt(String value, int defaultValue) {
        try {
            return Integer.parseInt(value.trim());
        } catch (NumberFormatException error) {
            return defaultValue;
        }
    }

    private void loadProperties(String encoded) {
        synchronized (properties) {
            properties.clear();
            StringBuilder entry = new StringBuilder();
            for (int index = 0; index <= encoded.length(); index++) {
                if (index == encoded.length() || encoded.charAt(index) == '\t') {
                    if (index < encoded.length()
                            && index + 1 < encoded.length()
                            && encoded.charAt(index + 1) == '\t') {
                        entry.append('\t');
                        index++;
                        continue;
                    }
                    addProperty(entry.toString());
                    entry.setLength(0);
                } else {
                    entry.append(encoded.charAt(index));
                }
            }
        }
    }

    private void addProperty(String entry) {
        int equals = entry.indexOf('=');
        if (equals >= 0) {
            properties.setProperty(entry.substring(0, equals), entry.substring(equals + 1));
        }
    }

    private long longProperty(String name, long defaultValue) {
        synchronized (properties) {
            String value = properties.getProperty(name);
            if (value == null) {
                return defaultValue;
            }
            try {
                return Long.parseLong(value.trim());
            } catch (NumberFormatException error) {
                return defaultValue;
            }
        }
    }

    private long pingTimeoutMillis() {
        long timeoutSeconds = longProperty("wrapper.ping.timeout", 30L);
        long intervalSeconds = longProperty("wrapper.ping.interval", 5L);
        if (timeoutSeconds <= 0L) {
            return 0L;
        }
        return (timeoutSeconds + Math.max(intervalSeconds, 1L)) * 1_000L;
    }

    private boolean pingTimedOut() {
        long timeout = pingTimeoutMillis();
        return timeout > 0L && System.currentTimeMillis() - lastWrapperPacketMillis > timeout;
    }

    private synchronized void closeSocket() throws IOException {
        input = null;
        output = null;
        if (socket != null) {
            try {
                socket.close();
            } finally {
                socket = null;
            }
        }
    }

    private void closeSocketQuietly() {
        try {
            closeSocket();
        } catch (IOException ignored) {
            // Best effort during JVM shutdown.
        }
    }

    private static final class Packet {
        private final int code;
        private final String message;

        private Packet(int code, String message) {
            this.code = code;
            this.message = message;
        }
    }
}
