// SPDX-License-Identifier: Apache-2.0 OR MIT
import io.github.jayyanez.jss.bridge.Steward;

/** Exercises the Steward control API: identifiers, logging, stop and restart. */
public final class ControlApplication {
    private ControlApplication() {
    }

    public static void main(String[] arguments) throws Exception {
        System.out.println("JSS_CONTROL_READY");
        System.out.println("JSS_CONTROL_JVM_ID " + Steward.getJvmId());
        System.out.println("JSS_CONTROL_WRAPPER_PID " + Steward.getWrapperPid());
        System.out.println("JSS_CONTROL_JAVA_PID " + Steward.getJavaPid());
        System.out.println("JSS_CONTROL_LOGFILE " + Steward.getLogFile());
        System.out.println("JSS_CONTROL_VERSION " + Steward.getVersion());
        System.out.println("JSS_CONTROL_PROPERTY "
                + Steward.getProperties().getProperty("wrapper.ntservice.name"));
        System.out.println("JSS_CONTROL_MANAGED " + Steward.isManaged());
        Steward.log(Steward.LOG_INFO, "JSS_CONTROL_PROTOCOL_LOG");
        Thread.sleep(1_000L);
        if (arguments.length > 0 && "idle".equals(arguments[0])) {
            Thread.sleep(10_000L);
            Steward.stop(0);
            while (true) {
                Thread.sleep(1_000L);
            }
        }
        if (arguments.length > 0 && "restart".equals(arguments[0])) {
            Steward.restart();
            while (true) {
                Thread.sleep(1_000L);
            }
        }
        int exitCode = arguments.length == 0 ? 0 : Integer.parseInt(arguments[0]);
        Steward.stop(exitCode);
        while (true) {
            Thread.sleep(1_000L);
        }
    }
}
