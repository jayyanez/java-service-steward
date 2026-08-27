// SPDX-License-Identifier: Apache-2.0 OR MIT
/** Stop class for the StartStopApp launcher test. */
public final class StartStopApplicationStop {
    private StartStopApplicationStop() {
    }

    public static void main(String[] arguments) {
        System.out.println("JSS_STARTSTOP_STOP_INVOKED arguments=" + arguments.length);
        StartStopApplication.STOP_REQUESTED.countDown();
    }
}
