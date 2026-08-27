// SPDX-License-Identifier: Apache-2.0 OR MIT
package io.github.jayyanez.jss.bridge;

/**
 * Lifecycle callbacks for applications that integrate with the wrapper from
 * their own {@code main} method through {@link Steward#start(ServiceListener, String[])}.
 */
public interface ServiceListener {
    /**
     * Starts the application. Return {@code null} to keep running, or an exit
     * code to stop the JVM immediately with that code.
     */
    Integer start(String[] arguments);

    /**
     * Stops the application. {@code exitCode} is the code requested by the
     * wrapper; the returned value is the code the JVM will exit with.
     */
    int stop(int exitCode);

    /**
     * Receives control events: {@link Steward#CONTROL_PAUSE},
     * {@link Steward#CONTROL_RESUME}, or a user-defined Windows service control
     * code in the range 128-255 delivered unchanged.
     */
    void controlEvent(int event);
}
