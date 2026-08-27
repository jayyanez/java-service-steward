// SPDX-License-Identifier: Apache-2.0 OR MIT
import io.github.jayyanez.jss.bridge.ServiceListener;
import io.github.jayyanez.jss.bridge.Steward;

/** Custom main class that integrates through the ServiceListener callbacks. */
public final class ListenerApplication implements ServiceListener {
    private volatile boolean running = true;

    public static void main(String[] arguments) {
        Steward.start(new ListenerApplication(), arguments);
        System.out.println("JSS_LISTENER_MAIN_RETURNED");
    }

    @Override
    public Integer start(String[] arguments) {
        System.out.println("JSS_LISTENER_START arguments=" + arguments.length);
        Thread worker = new Thread(new Runnable() {
            @Override
            public void run() {
                while (running) {
                    try {
                        Thread.sleep(200L);
                    } catch (InterruptedException ignored) {
                        return;
                    }
                }
            }
        }, "listener-worker");
        worker.setDaemon(false);
        worker.start();
        return null;
    }

    @Override
    public int stop(int exitCode) {
        System.out.println("JSS_LISTENER_STOP_CALLED " + exitCode);
        running = false;
        return 3;
    }

    @Override
    public void controlEvent(int event) {
        System.out.println("JSS_LISTENER_CONTROL " + event);
    }
}
