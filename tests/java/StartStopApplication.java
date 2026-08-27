// SPDX-License-Identifier: Apache-2.0 OR MIT
import java.util.concurrent.CountDownLatch;

/** Start class for the StartStopApp launcher test. */
public final class StartStopApplication {
    static final CountDownLatch STOP_REQUESTED = new CountDownLatch(1);

    private StartStopApplication() {
    }

    public static void main(String[] arguments) throws Exception {
        System.out.println("JSS_STARTSTOP_READY arguments=" + arguments.length);
        Thread worker = new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    STOP_REQUESTED.await();
                    System.out.println("JSS_STARTSTOP_WORKER_FINISHED");
                } catch (InterruptedException ignored) {
                    Thread.currentThread().interrupt();
                }
            }
        }, "startstop-worker");
        worker.setDaemon(false);
        worker.start();
    }
}
